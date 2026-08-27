/*!
Linux PipeWire graph-rate discovery.

CPAL's Linux backend talks ALSA.  The PipeWire ALSA plugin accepts essentially
any client rate, so `default_output_config()` describes the virtual ALSA PCM,
not the PipeWire graph clock.  Query the `settings` metadata object only when
ALSA's resolved `pcm.default` is actually the PipeWire plugin; direct hardware,
JACK, and other ALSA routes keep using CPAL's own device selection.
*/

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};
use std::ptr;
use std::rc::Rc;
use std::time::Duration;

use ::pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::Listener as RegistryListener;
use pw::types::ObjectType;

const SETTINGS_METADATA_NAME: &str = "settings";
const CLOCK_RATE_KEY: &str = "clock.rate";
const CLOCK_FORCE_RATE_KEY: &str = "clock.force-rate";
const QUERY_TIMEOUT: Duration = Duration::from_millis(250);

/// Return the active PipeWire graph clock when ALSA's default output resolves
/// to the PipeWire PCM plugin.  Any unavailable server, malformed metadata, or
/// non-PipeWire route is a normal `None`: callers must retain their CPAL
/// fallback path.
pub(super) fn default_output_graph_rate() -> Option<u32> {
    if !default_alsa_pcm_is_pipewire() {
        return None;
    }
    query_graph_rate()
}

fn default_alsa_pcm_is_pipewire() -> bool {
    let base = CString::new("pcm").unwrap();
    let name = CString::new("default").unwrap();
    let type_key = CString::new("type").unwrap();
    let mut root = ptr::null_mut();

    unsafe {
        if alsa_sys::snd_config_update_ref(&mut root) < 0 || root.is_null() {
            return false;
        }

        let mut definition = ptr::null_mut();
        let definition_result = alsa_sys::snd_config_search_definition(
            root,
            base.as_ptr(),
            name.as_ptr(),
            &mut definition,
        );
        alsa_sys::snd_config_unref(root);
        if definition_result < 0 || definition.is_null() {
            return false;
        }

        let mut type_node = ptr::null_mut();
        let search_result =
            alsa_sys::snd_config_search(definition, type_key.as_ptr(), &mut type_node);
        let mut pcm_type = ptr::null();
        let string_result = if search_result >= 0 && !type_node.is_null() {
            alsa_sys::snd_config_get_string(type_node, &mut pcm_type)
        } else {
            -1
        };
        let is_pipewire = string_result >= 0
            && !pcm_type.is_null()
            && CStr::from_ptr(pcm_type).to_bytes() == b"pipewire";
        alsa_sys::snd_config_delete(definition);
        is_pipewire
    }
}

fn query_graph_rate() -> Option<u32> {
    pw::init();

    let main_loop = pw::main_loop::MainLoopRc::new(None).ok()?;
    let context = pw::context::ContextRc::new(&main_loop, None).ok()?;
    let core = context.connect_rc(None).ok()?;
    let registry = core.get_registry_rc().ok()?;

    let graph_rate = Rc::new(Cell::new(None));
    let forced_graph_rate = Rc::new(Cell::new(None));
    // A metadata proxy and its listener must both outlive the registry callback.
    let metadata_objects: Rc<RefCell<Vec<(Metadata, MetadataListener)>>> =
        Rc::new(RefCell::new(Vec::new()));

    let registry_weak = registry.downgrade();
    let graph_rate_for_registry = Rc::clone(&graph_rate);
    let forced_graph_rate_for_registry = Rc::clone(&forced_graph_rate);
    let metadata_objects_for_registry = Rc::clone(&metadata_objects);
    let main_loop_for_registry = main_loop.clone();
    let registry_listener: RegistryListener = registry
        .add_listener_local()
        .global(move |object| {
            if object.type_ != ObjectType::Metadata
                || object.props.and_then(|props| props.get("metadata.name"))
                    != Some(SETTINGS_METADATA_NAME)
            {
                return;
            }
            let Some(registry) = registry_weak.upgrade() else {
                return;
            };
            let metadata: Metadata = match registry.bind(object) {
                Ok(metadata) => metadata,
                Err(_) => return,
            };

            let graph_rate = Rc::clone(&graph_rate_for_registry);
            let forced_graph_rate = Rc::clone(&forced_graph_rate_for_registry);
            let main_loop_weak = main_loop_for_registry.downgrade();
            let listener = metadata
                .add_listener_local()
                .property(move |_subject, key, _type, value| {
                    let parsed = value.and_then(|value| value.parse::<u32>().ok());
                    match key {
                        Some(CLOCK_RATE_KEY) => graph_rate.set(parsed.filter(|rate| *rate > 0)),
                        // Zero explicitly means "no forced rate", so retain it
                        // as an observed value rather than treating it as absent.
                        Some(CLOCK_FORCE_RATE_KEY) => forced_graph_rate.set(parsed),
                        _ => {}
                    }
                    if graph_rate.get().is_some() && forced_graph_rate.get().is_some() {
                        if let Some(main_loop) = main_loop_weak.upgrade() {
                            main_loop.quit();
                        }
                    }
                    0
                })
                .register();
            metadata_objects_for_registry
                .borrow_mut()
                .push((metadata, listener));
        })
        .register();

    let timeout_loop = main_loop.downgrade();
    let timeout = main_loop.loop_().add_timer(move |_| {
        if let Some(main_loop) = timeout_loop.upgrade() {
            main_loop.quit();
        }
    });
    if timeout
        .update_timer(Some(QUERY_TIMEOUT), None)
        .into_result()
        .is_err()
    {
        return None;
    }
    main_loop.run();

    // Keep all event owners alive through run(), then make the destruction
    // order explicit: listener before registry/proxies before loop/context.
    drop(registry_listener);
    drop(metadata_objects);
    forced_graph_rate
        .get()
        .filter(|rate| *rate > 0)
        .or_else(|| graph_rate.get())
}

#[cfg(test)]
mod tests {
    use super::{default_alsa_pcm_is_pipewire, default_output_graph_rate};

    #[test]
    fn resolved_default_alsa_pcm_probe_does_not_panic() {
        // The assertion is intentionally environment-neutral: CI may route
        // default to PipeWire, direct ALSA, JACK, or no configured PCM.
        let _ = default_alsa_pcm_is_pipewire();
    }

    #[test]
    #[ignore = "requires pcm.default routed to a running PipeWire server"]
    fn linux_pipewire_default_exposes_graph_rate() {
        assert!(default_alsa_pcm_is_pipewire(), "pcm.default is not PipeWire");
        let rate = default_output_graph_rate()
            .expect("PipeWire settings metadata has no clock.rate");
        eprintln!("PipeWire graph rate: {rate} Hz");
        assert!((8_000..=384_000).contains(&rate), "implausible graph rate: {rate}");
    }
}
