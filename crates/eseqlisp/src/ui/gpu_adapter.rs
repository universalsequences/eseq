//! Startup reporting and assertion for the wgpu adapter the app actually got.
//!
//! `wgpu` will happily hand back a working adapter that is not the one the
//! renderer is tuned for. On Linux the common case is a missing or broken
//! Vulkan ICD: instead of failing, wgpu falls back to OpenGL, or to a software
//! rasterizer (llvmpipe/lavapipe) that reports itself as a normal device. Both
//! render correct pixels and both have entirely different performance and
//! feature characteristics, so a silent fallback turns "the port is slow" into
//! an unfalsifiable claim.
//!
//! Startup therefore does two things: it logs exactly which adapter and backend
//! were selected, and it *asserts* that the selection is one the renderer is
//! meant to run on. The policy is a pure function of `wgpu::AdapterInfo` plus
//! two environment overrides, so it is unit-testable without a GPU.

/// Name the interactive app requires the selected backend to report.
///
/// Set to a `wgpu::Backend` name (`vulkan`, `metal`, `dx12`, `gl`, `webgpu`) to
/// pin the backend, or to `any` to accept whatever was selected. Unset applies
/// the default policy below.
pub const REQUIRE_BACKEND_ENV: &str = "ESEQ_GPU_BACKEND";

/// Set to any value to downgrade the default fallback rejections to a warning.
pub const ALLOW_FALLBACK_ENV: &str = "ESEQ_ALLOW_GPU_FALLBACK";

/// Why a selected adapter was rejected. Carried separately from the message so
/// callers can decide between hard-failing and warning without string matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterRejection {
    /// The backend did not match an explicit [`REQUIRE_BACKEND_ENV`] pin.
    BackendMismatch,
    /// wgpu fell back to OpenGL, which on Linux means no usable Vulkan ICD.
    OpenGlFallback,
    /// The adapter is a CPU rasterizer, not a GPU.
    SoftwareRasterizer,
    /// wgpu reported the dummy backend; nothing will render.
    EmptyBackend,
}

impl AdapterRejection {
    /// Operator-facing explanation, including the escape hatch that permits the
    /// selection anyway. Kept next to the variant so every caller says the same
    /// thing.
    pub fn advice(self) -> &'static str {
        match self {
            AdapterRejection::BackendMismatch => {
                "unset or correct ESEQ_GPU_BACKEND, or set ESEQ_GPU_BACKEND=any"
            }
            AdapterRejection::OpenGlFallback => {
                "install a Vulkan driver for this GPU (mesa's vulkan-intel, vulkan-radeon, \
                 or nvidia-utils) and confirm it with vulkaninfo; set \
                 ESEQ_ALLOW_GPU_FALLBACK=1 to run on OpenGL anyway"
            }
            AdapterRejection::SoftwareRasterizer => {
                "no hardware adapter was found, so wgpu selected a CPU rasterizer; set \
                 ESEQ_ALLOW_GPU_FALLBACK=1 to run on it anyway"
            }
            AdapterRejection::EmptyBackend => {
                "wgpu reported no usable graphics backend on this system"
            }
        }
    }
}

/// The two environment overrides, resolved once so tests can supply them
/// directly instead of mutating process environment.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AdapterPolicy {
    /// Value of [`REQUIRE_BACKEND_ENV`], lowercased, if set.
    pub required_backend: Option<String>,
    /// Whether [`ALLOW_FALLBACK_ENV`] is set.
    pub allow_fallback: bool,
}

impl AdapterPolicy {
    /// Read the policy from the process environment.
    pub fn from_env() -> Self {
        Self {
            required_backend: std::env::var(REQUIRE_BACKEND_ENV)
                .ok()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty()),
            allow_fallback: std::env::var_os(ALLOW_FALLBACK_ENV).is_some(),
        }
    }
}

/// One-line description of the selected adapter, used for the startup log and
/// embedded in rejection messages so the two always agree.
pub fn describe(info: &wgpu::AdapterInfo) -> String {
    format!(
        "{backend} adapter {name:?} ({device_type:?}, driver {driver:?} {driver_info:?})",
        backend = info.backend.to_str(),
        name = info.name,
        device_type = info.device_type,
        driver = info.driver,
        driver_info = info.driver_info,
    )
}

/// Decide whether the selected adapter is one the renderer is meant to run on.
///
/// An explicit backend pin is checked first and is the only thing that can
/// reject an otherwise-fine hardware adapter. Otherwise the default policy
/// rejects exactly the silent degradations: the OpenGL fallback, a CPU
/// rasterizer, and the dummy backend. `allow_fallback` suppresses the defaults
/// but never the explicit pin, because a pin is a direct instruction.
pub fn evaluate(info: &wgpu::AdapterInfo, policy: &AdapterPolicy) -> Result<(), AdapterRejection> {
    if let Some(required) = policy.required_backend.as_deref() {
        if required == "any" {
            return Ok(());
        }
        if required != info.backend.to_str() {
            return Err(AdapterRejection::BackendMismatch);
        }
        return Ok(());
    }
    if info.backend == wgpu::Backend::Empty {
        return Err(AdapterRejection::EmptyBackend);
    }
    if policy.allow_fallback {
        return Ok(());
    }
    if info.backend == wgpu::Backend::Gl {
        return Err(AdapterRejection::OpenGlFallback);
    }
    if info.device_type == wgpu::DeviceType::Cpu {
        return Err(AdapterRejection::SoftwareRasterizer);
    }
    Ok(())
}

/// Full rejection message: what was selected, what was expected, and how to
/// proceed.
pub fn rejection_message(
    info: &wgpu::AdapterInfo,
    policy: &AdapterPolicy,
    rejection: AdapterRejection,
) -> String {
    let expected = match policy.required_backend.as_deref() {
        Some(required) => format!("{REQUIRE_BACKEND_ENV}={required}"),
        None => "a hardware GPU backend".to_string(),
    };
    format!(
        "eseq: refusing to start on {selected}: expected {expected}; {advice}",
        selected = describe(info),
        advice = rejection.advice(),
    )
}

/// Log the selected adapter and assert it satisfies the policy.
///
/// Returns the rejection message when the selection is unacceptable; the caller
/// turns that into its own backend error so this module stays free of backend
/// types.
pub fn report_and_check(info: &wgpu::AdapterInfo, policy: &AdapterPolicy) -> Result<(), String> {
    eprintln!("eseq: selected {}", describe(info));
    match evaluate(info, policy) {
        Ok(()) => Ok(()),
        Err(rejection) => Err(rejection_message(info, policy, rejection)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(backend: wgpu::Backend, device_type: wgpu::DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "Test Adapter".to_string(),
            vendor: 0x8086,
            device: 0x5917,
            device_type,
            driver: "test".to_string(),
            driver_info: "test 1.0".to_string(),
            backend,
        }
    }

    fn policy(required: Option<&str>, allow_fallback: bool) -> AdapterPolicy {
        AdapterPolicy {
            required_backend: required.map(str::to_string),
            allow_fallback,
        }
    }

    #[test]
    fn hardware_vulkan_is_accepted_by_default() {
        let info = adapter(wgpu::Backend::Vulkan, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(evaluate(&info, &policy(None, false)), Ok(()));
    }

    #[test]
    fn opengl_fallback_is_rejected_by_default() {
        let info = adapter(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(
            evaluate(&info, &policy(None, false)),
            Err(AdapterRejection::OpenGlFallback)
        );
    }

    #[test]
    fn software_rasterizer_is_rejected_by_default() {
        let info = adapter(wgpu::Backend::Vulkan, wgpu::DeviceType::Cpu);
        assert_eq!(
            evaluate(&info, &policy(None, false)),
            Err(AdapterRejection::SoftwareRasterizer)
        );
    }

    #[test]
    fn fallback_override_permits_opengl_and_software() {
        let gl = adapter(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu);
        let cpu = adapter(wgpu::Backend::Vulkan, wgpu::DeviceType::Cpu);
        assert_eq!(evaluate(&gl, &policy(None, true)), Ok(()));
        assert_eq!(evaluate(&cpu, &policy(None, true)), Ok(()));
    }

    #[test]
    fn empty_backend_is_rejected_even_with_the_fallback_override() {
        let info = adapter(wgpu::Backend::Empty, wgpu::DeviceType::Other);
        assert_eq!(
            evaluate(&info, &policy(None, true)),
            Err(AdapterRejection::EmptyBackend)
        );
    }

    #[test]
    fn an_explicit_pin_rejects_every_other_backend() {
        let info = adapter(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(
            evaluate(&info, &policy(Some("vulkan"), false)),
            Err(AdapterRejection::BackendMismatch)
        );
        assert_eq!(evaluate(&info, &policy(Some("gl"), false)), Ok(()));
    }

    /// A pin is a direct instruction, so the fallback override must not be able
    /// to talk the check out of it.
    #[test]
    fn the_fallback_override_cannot_satisfy_an_explicit_pin() {
        let info = adapter(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(
            evaluate(&info, &policy(Some("vulkan"), true)),
            Err(AdapterRejection::BackendMismatch)
        );
    }

    #[test]
    fn any_accepts_a_selection_the_default_policy_would_reject() {
        let info = adapter(wgpu::Backend::Gl, wgpu::DeviceType::Cpu);
        assert_eq!(evaluate(&info, &policy(Some("any"), false)), Ok(()));
    }

    #[test]
    fn the_rejection_message_names_the_selection_and_the_escape_hatch() {
        let info = adapter(wgpu::Backend::Gl, wgpu::DeviceType::IntegratedGpu);
        let policy = policy(None, false);
        let message = rejection_message(&info, &policy, AdapterRejection::OpenGlFallback);
        assert!(message.contains("gl adapter"), "{message}");
        assert!(message.contains("ESEQ_ALLOW_GPU_FALLBACK=1"), "{message}");
    }
}
