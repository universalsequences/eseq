// VST3 smoke host for exported eseq plugins: loads a .vst3 bundle through the
// Steinberg hosting classes (the copy vendored by clap-wrapper), lists params,
// plays notes, checks the audio, round-trips state, and optionally opens the
// editor in a window for a few seconds.
//
//   crates/eseq-plugin/tools/build_vst3_smoke.sh   # builds ./vst3_smoke
//   vst3_smoke "<path>/X.vst3" [--gui SECONDS]

#import <Cocoa/Cocoa.h>

#include "public.sdk/source/vst/hosting/module.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include "public.sdk/source/vst/hosting/processdata.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/utility/stringconvert.h"
#include "public.sdk/source/vst/hosting/plugprovider.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/gui/iplugview.h"

#include <cmath>
#include <cstdio>
#include <vector>

using namespace Steinberg;
using namespace Steinberg::Vst;

static int fail(const char* what) {
    fprintf(stderr, "FAIL: %s\n", what);
    return 1;
}

int main(int argc, char** argv) {
    if (argc < 2) return fail("usage: vst3_smoke <bundle.vst3> [--gui SECONDS]");
    double gui_seconds = 0;
    const char* shot = nullptr;
    for (int i = 2; i + 1 < argc; ++i) {
        if (strcmp(argv[i], "--gui") == 0) gui_seconds = atof(argv[i + 1]);
        if (strcmp(argv[i], "--shot") == 0) shot = argv[i + 1];
    }

    std::string error;
    auto module = VST3::Hosting::Module::create(argv[1], error);
    if (!module) {
        fprintf(stderr, "module: %s\n", error.c_str());
        return fail("load module");
    }
    auto host = owned(new HostApplication());
    PluginContextFactory::instance().setPluginContext(host);
    const auto& factory = module->getFactory();
    VST3::Hosting::ClassInfo audio;
    bool found = false;
    for (auto& info : factory.classInfos()) {
        printf("class: %s [%s] vendor=%s sub=%s\n", info.name().c_str(), info.category().c_str(),
               info.vendor().c_str(), info.subCategoriesString().c_str());
        if (info.category() == kVstAudioEffectClass) { audio = info; found = true; }
    }
    if (!found) return fail("no audio class");
    auto component = factory.createInstance<IComponent>(audio.ID());
    if (!component || component->initialize(host) != kResultOk) return fail("component init");

    IPtr<IEditController> controller;
    if (component->queryInterface(IEditController::iid, (void**)&controller) != kResultOk || !controller) {
        TUID cid;
        if (component->getControllerClassId(cid) == kResultOk) {
            controller = factory.createInstance<IEditController>(VST3::UID::fromTUID(cid));
            if (controller) controller->initialize(host);
        }
    }
    if (!controller) return fail("edit controller");
    int32 nparams = controller->getParameterCount();
    printf("params: %d\n", nparams);
    for (int32 i = 0; i < nparams && i < 5; ++i) {
        ParameterInfo info{};
        controller->getParameterInfo(i, info);
        String128 text{};
        controller->getParamStringByValue(info.id, controller->getParamNormalized(info.id), text);
        printf("  %u %s = %s\n", info.id, VST3::StringConvert::convert(info.title).c_str(),
               VST3::StringConvert::convert(text).c_str());
    }

    FUnknownPtr<IAudioProcessor> processor(component);
    if (!processor) return fail("audio processor");
    const int32 block = 256;
    const double sr = 48000.0;
    ProcessSetup setup{kRealtime, kSample32, block, sr};
    if (processor->setupProcessing(setup) != kResultOk) return fail("setupProcessing");
    for (int32 i = 0; i < component->getBusCount(kAudio, kOutput); ++i)
        component->activateBus(kAudio, kOutput, i, true);
    for (int32 i = 0; i < component->getBusCount(kEvent, kInput); ++i)
        component->activateBus(kEvent, kInput, i, true);
    if (component->setActive(true) != kResultOk) return fail("setActive");
    processor->setProcessing(true);

    HostProcessData data;
    data.prepare(*component, block, kSample32);
    EventList events;
    ParameterChanges inParams, outParams;
    data.inputEvents = &events;
    data.inputParameterChanges = &inParams;
    data.outputParameterChanges = &outParams;
    data.numSamples = block;
    const int frames = (int)(sr * 1.0);
    std::vector<float> left;
    for (int done = 0; done < frames; done += block) {
        events.clear();
        if (done == 0) {
            Event e{};
            e.type = Event::kNoteOnEvent;
            e.noteOn.pitch = 36;
            e.noteOn.velocity = 0.9f;
            e.noteOn.noteId = -1;
            events.addEvent(e);
        }
        if (done == 12032) {
            Event e{};
            e.type = Event::kNoteOffEvent;
            e.noteOff.pitch = 36;
            e.noteOff.noteId = -1;
            events.addEvent(e);
        }
        if (processor->process(data) != kResultOk) return fail("process");
        float* l = data.outputs[0].channelBuffers32[0];
        left.insert(left.end(), l, l + block);
    }
    float peak = 0;
    double sum = 0;
    int bad = 0;
    for (float s : left) {
        if (!std::isfinite(s)) { ++bad; continue; }
        peak = std::max(peak, std::fabs(s));
        sum += s * s;
    }
    printf("note 36: peak %.4f rms %.4f nonfinite %d\n", peak, std::sqrt(sum / left.size()), bad);
    if (peak < 1e-3 || bad) return fail("no sound");

    auto stream = owned(new MemoryStream());
    if (component->getState(stream) != kResultOk) return fail("getState");
    int64 size = 0;
    stream->seek(0, IBStream::kIBSeekEnd, &size);
    stream->seek(0, IBStream::kIBSeekSet, nullptr);
    printf("state: %lld bytes\n", size);
    if (component->setState(stream) != kResultOk) return fail("setState");

    if (gui_seconds > 0) {
        IPtr<IPlugView> view = owned(controller->createView(ViewType::kEditor));
        if (!view) return fail("createView");
        if (view->isPlatformTypeSupported(kPlatformTypeNSView) != kResultTrue) return fail("NSView unsupported");
        ViewRect rect{};
        view->getSize(&rect);
        printf("editor: %dx%d\n", rect.getWidth(), rect.getHeight());
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow* window = [[NSWindow alloc]
            initWithContentRect:NSMakeRect(200, 200, rect.getWidth(), rect.getHeight())
                      styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [window setTitle:@"vst3_smoke"];
        if (view->attached((__bridge void*)[window contentView], kPlatformTypeNSView) != kResultOk)
            return fail("attached");
        [window makeKeyAndOrderFront:nil];
        [NSApp activateIgnoringOtherApps:YES];
        NSDate* until = [NSDate dateWithTimeIntervalSinceNow:gui_seconds];
        bool shot_taken = false;
        while ([until timeIntervalSinceNow] > 0) {
            if (shot && !shot_taken && [until timeIntervalSinceNow] < gui_seconds / 2) {
                char cmd[1024];
                snprintf(cmd, sizeof cmd, "/usr/sbin/screencapture -x -o -l %ld '%s'", (long)[window windowNumber], shot);
                printf("screenshot: %s (status %d)\n", shot, system(cmd));
                shot_taken = true;
            }
            @autoreleasepool {
                NSEvent* event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                                    untilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]
                                                       inMode:NSDefaultRunLoopMode
                                                      dequeue:YES];
                if (event) [NSApp sendEvent:event];
            }
        }
        view->removed();
        [window close];
        printf("editor: opened and closed\n");
    }

    processor->setProcessing(false);
    component->setActive(false);
    data.unprepare();
    controller->terminate();
    component->terminate();
    PluginContextFactory::instance().setPluginContext(nullptr);
    printf("OK\n");
    return 0;
}
