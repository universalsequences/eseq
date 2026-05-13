# AudioGraph Swift Integration Guide

This guide shows how to integrate the audiograph dylib into your Swift project for real-time audio processing.

## Quick Start: Running the Example

To quickly test the integration and hear the audiograph in action:

### 1. Build the Dynamic Library
```bash
# From the audiograph directory
make lib-release
```

### 2. Create Example Project Directory
```bash
mkdir MyAudioProject
cd MyAudioProject
mkdir -p Sources/MyAudioProject
mkdir audiograph
```

### 3. Copy Required Files
Copy these files from the audiograph directory to your project:
```bash
# Copy the dynamic library
cp /path/to/audiograph/libaudiograph.dylib ./audiograph/

# Copy all header files
cp /path/to/audiograph/audiograph_swift.h ./audiograph/
cp /path/to/audiograph/graph_types.h ./audiograph/
cp /path/to/audiograph/graph_engine.h ./audiograph/
cp /path/to/audiograph/graph_api.h ./audiograph/
cp /path/to/audiograph/graph_edit.h ./audiograph/
cp /path/to/audiograph/graph_nodes.h ./audiograph/
```

### 4. Create Package.swift
```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyAudioProject",
    platforms: [
        .macOS(.v10_15)
    ],
    targets: [
        .executableTarget(
            name: "MyAudioProject",
            dependencies: ["AudioGraph"],
            linkerSettings: [
                .linkedLibrary("audiograph"),
                .unsafeFlags(["-L", "./audiograph"])
            ]
        ),
        .systemLibrary(
            name: "AudioGraph",
            path: "audiograph"
        )
    ]
)
```

### 5. Create module.modulemap
Create `audiograph/module.modulemap`:
```
module AudioGraph {
    header "audiograph_swift.h"
    link "audiograph"
    export *
}
```

### 6. Create main.swift
Create `Sources/MyAudioProject/main.swift` with the complete working example (see Section 5 below for the full stereo audio code).

### 7. Build and Run
```bash
# Build the project
DYLD_LIBRARY_PATH="$PWD/audiograph" swift build

# Run and hear the C major chord
DYLD_LIBRARY_PATH="$PWD/audiograph" swift run
```

**Expected Output**: You should hear a pleasant C major chord (4 oscillators) playing for 5 seconds in stereo.

### Alternative: Copy Working Example
If you want to use the pre-built example from this repository:
```bash
# Copy the entire working example
cp -r /path/to/audiograph/swift_example ./MyAudioProject

# Update the dylib and build
cd MyAudioProject
cp ../libaudiograph.dylib ./audiograph/
DYLD_LIBRARY_PATH="$PWD/audiograph" swift run
```

### File Checklist
Before building, ensure your project directory contains:
```
MyAudioProject/
├── Package.swift                    ✓ Created in step 4
├── Sources/
│   └── MyAudioProject/
│       └── main.swift              ✓ Created in step 6 (full example below)
├── audiograph/
│   ├── libaudiograph.dylib         ✓ Copied from audiograph directory
│   ├── module.modulemap            ✓ Created in step 5
│   ├── audiograph_swift.h          ✓ Copied from audiograph directory
│   ├── graph_types.h               ✓ Copied from audiograph directory
│   ├── graph_engine.h              ✓ Copied from audiograph directory
│   ├── graph_api.h                 ✓ Copied from audiograph directory
│   ├── graph_edit.h                ✓ Copied from audiograph directory
│   └── graph_nodes.h               ✓ Copied from audiograph directory
```

### Complete Copy-Paste Script
Here's a complete script you can run from the audiograph directory to set up and test the example:

```bash
#!/bin/bash
# Complete setup script for audiograph Swift integration

# 1. Build the library
echo "Building audiograph library..."
make lib-release

# 2. Create project structure
echo "Creating project structure..."
mkdir -p TestAudioProject/Sources/TestAudioProject
mkdir -p TestAudioProject/audiograph
cd TestAudioProject

# 3. Copy library and headers
echo "Copying library files..."
cp ../libaudiograph.dylib ./audiograph/
cp ../audiograph_swift.h ./audiograph/
cp ../graph_types.h ./audiograph/
cp ../graph_engine.h ./audiograph/
cp ../graph_api.h ./audiograph/
cp ../graph_edit.h ./audiograph/
cp ../graph_nodes.h ./audiograph/

# 4. Create Package.swift
cat > Package.swift << 'EOF'
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "TestAudioProject",
    platforms: [
        .macOS(.v10_15)
    ],
    targets: [
        .executableTarget(
            name: "TestAudioProject",
            dependencies: ["AudioGraph"],
            linkerSettings: [
                .linkedLibrary("audiograph"),
                .unsafeFlags(["-L", "./audiograph"])
            ]
        ),
        .systemLibrary(
            name: "AudioGraph",
            path: "audiograph"
        )
    ]
)
EOF

# 5. Create module map
cat > audiograph/module.modulemap << 'EOF'
module AudioGraph {
    header "audiograph_swift.h"
    link "audiograph"
    export *
}
EOF

# 6. Copy working main.swift from our example
cp ../swift_example/Sources/AudioGraphExample/main.swift ./Sources/TestAudioProject/main.swift

# 7. Build and run
echo "Building and running test..."
DYLD_LIBRARY_PATH="$PWD/audiograph" swift build
echo "Starting audio test - you should hear a C major chord!"
DYLD_LIBRARY_PATH="$PWD/audiograph" swift run

echo "Complete! Your audiograph Swift integration is working."
```

Save this as `setup_swift_example.sh`, make it executable with `chmod +x setup_swift_example.sh`, and run it.

**OR** simply use the provided `setup_swift_example.sh` script in the audiograph directory:
```bash
./setup_swift_example.sh
```

## Building the Dynamic Library

### 1. Compile the Library
```bash
# Build optimized release version
make lib-release

# This creates: libaudiograph.dylib
```

### 2. Copy Library Files to Your Swift Project
Copy these files to your Swift project:
- `libaudiograph.dylib` - The dynamic library
- `audiograph_swift.h` - Swift-compatible header file
- All `.h` files (`graph_types.h`, `graph_engine.h`, etc.) - Required for compilation

## Swift Project Setup

There are two main ways to integrate audiograph into your Swift project: via Xcode directly or using Swift command-line tools (`swift build`, `swift run`).

## Option A: Swift Command-Line Tools (Package.swift)

This approach is perfect for command-line Swift projects, server-side Swift, or when you prefer working outside of Xcode.

### 1. Project Structure
Set up your Swift project directory like this:
```
MyAudioProject/
├── Package.swift
├── Sources/
│   └── MyAudioProject/
│       └── main.swift
├── audiograph/
│   ├── libaudiograph.dylib
│   ├── audiograph_swift.h
│   ├── graph_types.h
│   ├── graph_engine.h
│   ├── graph_api.h
│   ├── graph_edit.h
│   └── graph_nodes.h
└── module.modulemap
```

### 2. Create Package.swift
```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyAudioProject",
    platforms: [
        .macOS(.v10_15)
    ],
    targets: [
        .executableTarget(
            name: "MyAudioProject",
            dependencies: ["AudioGraph"]
        ),
        .systemLibrary(
            name: "AudioGraph",
            path: "audiograph",
            pkgConfig: "audiograph",
            providers: [
                .brew(["audiograph"])  // Optional: if you plan to distribute via brew
            ]
        )
    ]
)
```

### 3. Create module.modulemap
Create `audiograph/module.modulemap`:
```
module AudioGraph {
    header "audiograph_swift.h"
    link "audiograph"
    export *
}
```

### 4. Configure Library Search Path
You have several options for linking:

#### Option 4a: Copy dylib to system location
```bash
# Copy to system library directory (requires sudo)
sudo cp libaudiograph.dylib /usr/local/lib/

# Or copy to user library directory
mkdir -p ~/lib
cp libaudiograph.dylib ~/lib/
```

#### Option 4b: Use environment variables
```bash
# Set library path for current session
export DYLD_LIBRARY_PATH="$PWD/audiograph:$DYLD_LIBRARY_PATH"

# Then build/run normally
swift build
swift run
```

#### Option 4c: Use linker flags in Package.swift
```swift
.executableTarget(
    name: "MyAudioProject",
    dependencies: ["AudioGraph"],
    linkerSettings: [
        .linkedLibrary("audiograph"),
        .unsafeFlags(["-L", "./audiograph"])
    ]
)
```

### 5. Example main.swift with Real Audio Output

This example creates a real-time audio application using AVAudioSourceNode to integrate audiograph with macOS audio output:

```swift
import AudioGraph
import AVFoundation
import Foundation

// Global audio graph manager for use in render callback
class AudioGraphManager {
    private var liveGraph: UnsafeMutablePointer<LiveGraph>?
    private let blockSize: Int32 = 512
    private var audioBuffer = [Float]()
    
    init() {
        setupAudioGraph()
    }
    
    private func setupAudioGraph() {
        // Initialize engine with matching sample rate
        initialize_engine(blockSize, 44100)

        // Create mono graph (use 2 for native stereo support)
        guard let lg = create_live_graph(32, blockSize, "swift_av_graph", 1) else {
            print("✗ Failed to create live graph")
            return
        }
        
        liveGraph = lg
        audioBuffer = [Float](repeating: 0.0, count: Int(blockSize))
        
        // Start worker threads
        engine_start_workers(2)
        
        // Create multiple oscillators for a chord
        let frequencies: [Float] = [261.63, 329.63, 392.00, 523.25] // C major chord
        let gainPerOsc: Float = 0.15  // Adjusted for 4 oscillators
        
        var oscNodes: [Int32] = []
        var gainNodes: [Int32] = []
        
        // Create oscillators and individual gain controls
        for (i, freq) in frequencies.enumerated() {
            let osc = live_add_oscillator(lg, freq, "osc_\(i)")
            let gain = live_add_gain(lg, gainPerOsc, "gain_\(i)")
            oscNodes.append(osc)
            gainNodes.append(gain)
            
            // Connect osc -> gain
            _ = connect(lg, osc, 0, gain, 0)
        }
        
        // Create master mixer and master gain
        let mixer = live_add_mixer8(lg, "master_mix")
        let masterGain = live_add_gain(lg, 0.5, "master_vol")
        
        // Connect all gains to mixer inputs
        for (i, gainNode) in gainNodes.enumerated() {
            _ = connect(lg, gainNode, 0, mixer, Int32(i))
        }
        
        // Connect mixer -> master gain -> DAC
        _ = connect(lg, mixer, 0, masterGain, 0)
        _ = connect(lg, masterGain, 0, lg.pointee.dac_node_id, 0)
        
        print("✓ Created audio graph: 4 oscillators -> individual gains -> mixer -> master gain -> output")
        print("✓ Frequencies: \(frequencies.map { "\($0)Hz" }.joined(separator: ", "))")
    }
    
    func renderAudio(frameCount: UInt32, audioBufferList: UnsafeMutablePointer<AudioBufferList>) -> OSStatus {
        guard let lg = liveGraph else { return kAudioUnitErr_Uninitialized }
        
        let ablPointer = UnsafeMutableAudioBufferListPointer(audioBufferList)
        guard let leftBuffer = ablPointer[0].mData?.assumingMemoryBound(to: Float.self),
              let rightBuffer = ablPointer[1].mData?.assumingMemoryBound(to: Float.self) else {
            return kAudioUnitErr_InvalidParameter
        }
        
        var framesProcessed: UInt32 = 0
        
        // Process in chunks of our block size
        while framesProcessed < frameCount {
            let framesToProcess = min(UInt32(blockSize), frameCount - framesProcessed)
            
            // Get audio from audiograph (mono output)
            audioBuffer.withUnsafeMutableBufferPointer { bufferPtr in
                process_next_block(lg, bufferPtr.baseAddress!, Int32(framesToProcess))
            }
            
            // Copy mono signal to both stereo channels
            for i in 0..<Int(framesToProcess) {
                let sample = audioBuffer[i]
                leftBuffer[Int(framesProcessed) + i] = sample
                rightBuffer[Int(framesProcessed) + i] = sample
            }
            
            framesProcessed += framesToProcess
        }
        
        return noErr
    }
    
    deinit {
        engine_stop_workers()
        if let lg = liveGraph {
            destroy_live_graph(lg)
        }
    }
}

class AudioGraphSourceNode: AVAudioSourceNode {
    private let audioGraphManager = AudioGraphManager()
    
    init() {
        // Initialize with stereo format at 44.1kHz (standard audio)  
        let format = AVAudioFormat(standardFormatWithSampleRate: 44100, channels: 2)!
        
        super.init(format: format) { [weak audioGraphManager] _, _, frameCount, audioBufferList -> OSStatus in
            return audioGraphManager?.renderAudio(frameCount: frameCount, audioBufferList: audioBufferList) ?? kAudioUnitErr_Uninitialized
        }
    }
}

func main() {
    print("AudioGraph + AVAudioEngine Real-time Example")
    print("===========================================")
    
    let audioEngine = AVAudioEngine()
    let sourceNode = AudioGraphSourceNode()
    let mainMixer = audioEngine.mainMixerNode
    
    // Attach our custom source node
    audioEngine.attach(sourceNode)
    
    // Connect source -> mixer -> output
    audioEngine.connect(sourceNode, to: mainMixer, format: sourceNode.outputFormat(forBus: 0))
    audioEngine.connect(mainMixer, to: audioEngine.outputNode, format: nil)
    
    do {
        try audioEngine.start()
        print("✓ Audio engine started - you should hear a C major chord!")
        print("✓ Playing for 5 seconds...")
        
        // Play for 5 seconds
        Thread.sleep(forTimeInterval: 5.0)
        
        audioEngine.stop()
        print("✓ Audio stopped")
        
    } catch {
        print("✗ Audio engine error: \(error)")
    }
    
    print("\n✓ Real-time audio integration successful!")
}

main()
```

### 6. Build and Run
```bash
# Build the project (with library path)
DYLD_LIBRARY_PATH="$PWD/audiograph" swift build

# Run the executable (you should hear audio!)
DYLD_LIBRARY_PATH="$PWD/audiograph" swift run

# Build for release
DYLD_LIBRARY_PATH="$PWD/audiograph" swift build -c release

# Run release version
DYLD_LIBRARY_PATH="$PWD/audiograph" ./.build/release/MyAudioProject
```

**Important**: Always set `DYLD_LIBRARY_PATH` to include your audiograph directory so the dylib can be found at runtime.

### 7. Alternative: Direct Compilation
For simple projects, you can compile directly without Package.swift:
```bash
# Direct compilation with swift
swift -I ./audiograph -L ./audiograph -laudiograph main.swift -o myaudio

# Run with library path
DYLD_LIBRARY_PATH=./audiograph ./myaudio
```

### 8. Distribution Considerations
For distributing command-line tools:

```bash
# Bundle dylib with executable
mkdir -p MyAudioTool.app/Contents/{MacOS,Libraries}
cp .build/release/MyAudioProject MyAudioTool.app/Contents/MacOS/
cp audiograph/libaudiograph.dylib MyAudioTool.app/Contents/Libraries/

# Update install name to look in bundled location
install_name_tool -change @rpath/libaudiograph.dylib \
    @executable_path/../Libraries/libaudiograph.dylib \
    MyAudioTool.app/Contents/MacOS/MyAudioProject
```

## Option B: Xcode Project Integration

### 1. Add Library to Xcode Project
1. Drag `libaudiograph.dylib` into your Xcode project
2. Add to "Frameworks and Libraries" in your target settings
3. Set "Embed" to "Do Not Embed" (for dylib)

### 2. Create Bridging Header
Create a bridging header file (e.g., `YourProject-Bridging-Header.h`):
```c
#import "audiograph_swift.h"
```

### 3. Configure Build Settings
In your target build settings:
- Set "Objective-C Bridging Header" to your bridging header path
- Add library search path: `$(PROJECT_DIR)/path/to/dylib`
- Add header search path: `$(PROJECT_DIR)/path/to/headers`

### 4. Set Runtime Library Path
Add this to your target's "Runpath Search Paths":
```
@executable_path
@loader_path
```

## Swift Usage Example

```swift
import Foundation

class AudioGraphManager {
    private var liveGraph: OpaquePointer?
    private let blockSize: Int32 = 128
    private let sampleRate: Int32 = 48000
    
    init() {
        // Initialize the engine once
        initialize_engine(blockSize, sampleRate)

        // Create live graph (mono = 1 channel, stereo = 2 channels)
        liveGraph = create_live_graph(16, blockSize, "swift_graph", 1)
        
        // Start worker threads
        engine_start_workers(4)
    }
    
    deinit {
        engine_stop_workers()
        if let graph = liveGraph {
            destroy_live_graph(graph)
        }
    }
    
    func createOscillator(frequency: Float, name: String) -> Int32 {
        guard let graph = liveGraph else { return -1 }
        return live_add_oscillator(graph, frequency, name)
    }
    
    func createGain(value: Float, name: String) -> Int32 {
        guard let graph = liveGraph else { return -1 }
        return live_add_gain(graph, value, name)
    }
    
    func createMixer(name: String) -> Int32 {
        guard let graph = liveGraph else { return -1 }
        return live_add_mixer2(graph, name)
    }
    
    func connect(sourceNode: Int32, sourcePort: Int32, 
                 destNode: Int32, destPort: Int32) -> Bool {
        guard let graph = liveGraph else { return false }
        return connect(graph, sourceNode, sourcePort, destNode, destPort)
    }
    
    func disconnect(sourceNode: Int32, sourcePort: Int32,
                    destNode: Int32, destPort: Int32) -> Bool {
        guard let graph = liveGraph else { return false }
        return disconnect(graph, sourceNode, sourcePort, destNode, destPort)
    }
    
    func processAudioBlock() -> [Float] {
        guard let graph = liveGraph else { return [] }
        
        var outputBuffer = [Float](repeating: 0.0, count: Int(blockSize))
        outputBuffer.withUnsafeMutableBufferPointer { buffer in
            process_next_block(graph, buffer.baseAddress!, blockSize)
        }
        
        return outputBuffer
    }
    
    func updateParameter(nodeId: Int32, paramIndex: Int, value: Float) {
        guard let graph = liveGraph else { return }
        
        // Access the parameter ring buffer
        let paramMsg = ParamMsg(
            idx: UInt64(paramIndex),
            logical_id: UInt64(nodeId),
            fvalue: value
        )
        
        // Note: You'll need to access the params field of LiveGraph
        // This might require a helper function in C
        _ = params_push(graph.pointee.params, paramMsg)
    }
}
```

## Multi-Channel Audio Support

AudioGraph supports flexible channel configurations (mono, stereo, or more channels) with interleaved output format.

### Stereo Example

```swift
import AudioGraph

class StereoAudioGraphManager {
    private var liveGraph: OpaquePointer?
    private let blockSize: Int32 = 128

    init() {
        initialize_engine(blockSize, 48000)

        // Create stereo graph (2 channels)
        liveGraph = create_live_graph(16, blockSize, "stereo_graph", 2)

        guard let lg = liveGraph else { return }

        // Create separate oscillators for left and right channels
        let leftOsc = live_add_oscillator(lg, 440.0, "left_osc")   // A4
        let rightOsc = live_add_oscillator(lg, 554.37, "right_osc") // C#5

        // Connect to separate DAC channels
        _ = connect(lg, leftOsc, 0, lg.pointee.dac_node_id, 0)  // Left
        _ = connect(lg, rightOsc, 0, lg.pointee.dac_node_id, 1) // Right
    }

    func processAudioBlock() -> [Float] {
        guard let lg = liveGraph else { return [] }

        // Buffer size = nframes * num_channels for interleaved stereo
        var outputBuffer = [Float](repeating: 0.0, count: Int(blockSize) * 2)

        outputBuffer.withUnsafeMutableBufferPointer { bufferPtr in
            process_next_block(lg, bufferPtr.baseAddress!, blockSize)
        }

        // Output is interleaved: [L₀, R₀, L₁, R₁, L₂, R₂, ...]
        return outputBuffer
    }

    deinit {
        if let lg = liveGraph {
            destroy_live_graph(lg)
        }
    }
}
```

### Channel Configuration

```swift
// Mono (1 channel) - output buffer size = nframes
let monoGraph = create_live_graph(16, 128, "mono", 1)

// Stereo (2 channels) - output buffer size = nframes * 2
let stereoGraph = create_live_graph(16, 128, "stereo", 2)

// Quad (4 channels) - output buffer size = nframes * 4
let quadGraph = create_live_graph(16, 128, "quad", 4)
```

### Using Stereo Output with AVAudioEngine

```swift
func setupStereoAudio() {
    let audioEngine = AVAudioEngine()
    let outputNode = audioEngine.outputNode
    let format = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 2)!

    let sourceNode = AVAudioSourceNode(format: format) { _, _, frameCount, audioBufferList -> OSStatus in
        guard let lg = self.liveGraph else { return noErr }

        // Get interleaved stereo data from audiograph
        var interleavedBuffer = [Float](repeating: 0.0, count: Int(frameCount) * 2)
        interleavedBuffer.withUnsafeMutableBufferPointer { bufferPtr in
            process_next_block(lg, bufferPtr.baseAddress!, Int32(frameCount))
        }

        // De-interleave into separate L/R channel buffers for AVAudioEngine
        let ablPointer = UnsafeMutableAudioBufferListPointer(audioBufferList)
        guard let leftBuffer = ablPointer[0].mData?.assumingMemoryBound(to: Float.self),
              let rightBuffer = ablPointer[1].mData?.assumingMemoryBound(to: Float.self) else {
            return noErr
        }

        for i in 0..<Int(frameCount) {
            leftBuffer[i] = interleavedBuffer[i * 2 + 0]     // Left channel
            rightBuffer[i] = interleavedBuffer[i * 2 + 1]    // Right channel
        }

        return noErr
    }

    audioEngine.attach(sourceNode)
    audioEngine.connect(sourceNode, to: outputNode, format: format)
    try? audioEngine.start()
}
```

## Real-time Audio Integration

### With AVAudioEngine
```swift
import AVFoundation

class AudioGraphAVEngine {
    private let audioGraphManager = AudioGraphManager()
    private let audioEngine = AVAudioEngine()
    
    func startAudio() throws {
        let mainMixer = audioEngine.mainMixerNode
        let outputNode = audioEngine.outputNode
        let format = outputNode.inputFormat(forBus: 0)
        
        // Install a tap to process audio with audiograph
        mainMixer.installTap(onBus: 0, bufferSize: 128, format: format) { buffer, time in
            let audioBuffer = self.audioGraphManager.processAudioBlock()
            
            // Copy audiograph output to AVAudioPCMBuffer
            let frameLength = buffer.frameLength
            if let channelData = buffer.floatChannelData {
                for frame in 0..<Int(frameLength) {
                    if frame < audioBuffer.count {
                        channelData[0][frame] = audioBuffer[frame]
                    }
                }
            }
        }
        
        try audioEngine.start()
    }
    
    func stopAudio() {
        audioEngine.stop()
    }
}
```

### With Audio Unit
For lower-level integration, you can use the audiograph in an Audio Unit render callback:

```swift
let renderCallback: AURenderCallback = { (inRefCon, ioActionFlags, inTimeStamp, inBusNumber, inNumberFrames, ioData) -> OSStatus in
    
    guard let audioGraphManager = inRefCon?.assumingMemoryBound(to: AudioGraphManager.self).pointee else {
        return noErr
    }
    
    let audioBuffer = audioGraphManager.processAudioBlock()
    
    // Copy to output buffer
    guard let ioData = ioData,
          let buffers = ioData.pointee.mBuffers.mData?.assumingMemoryBound(to: Float.self) else {
        return noErr
    }
    
    for i in 0..<min(Int(inNumberFrames), audioBuffer.count) {
        buffers[i] = audioBuffer[i]
    }
    
    return noErr
}
```

## Memory Management Notes

### Important Considerations:
1. **Thread Safety**: Only `process_next_block()` and `params_push()` are real-time safe
2. **Memory Allocation**: All graph edits (`add_node`, `connect`, etc.) should be done from the main thread
3. **Lifecycle**: Always call `engine_stop_workers()` before destroying the live graph
4. **Parameter Updates**: Use the parameter ring buffer for real-time safe parameter changes

### Recommended Pattern:
```swift
// Main thread: Setup and graph editing
let oscId = audioGraphManager.createOscillator(frequency: 440.0, name: "A4")
let gainId = audioGraphManager.createGain(value: 0.5, name: "volume")
_ = audioGraphManager.connect(sourceNode: oscId, sourcePort: 0, 
                             destNode: gainId, destPort: 0)

// Audio thread: Only process audio
let samples = audioGraphManager.processAudioBlock()

// Any thread: Parameter updates (lock-free)
audioGraphManager.updateParameter(nodeId: gainId, paramIndex: 0, value: 0.8)
```

## Building for Distribution

### Static vs Dynamic Linking
- **Dynamic Library (.dylib)**: Easier to update, smaller app binary
- **Static Library (.a)**: Single binary, no external dependencies

### For App Store Distribution
Consider creating a static library version:
```bash
# Create static library
ar rcs libaudiograph.a graph_nodes.o graph_engine.o graph_api.o graph_edit.o
```

## Troubleshooting

### Common Issues (All Methods):
1. **Symbol not found**: Ensure bridging header includes `audiograph_swift.h`
2. **Runtime crashes**: Check that `initialize_engine()` is called before creating graphs
3. **Audio glitches**: Ensure audio processing happens only in `process_next_block()`
4. **Memory leaks**: Always pair `create_live_graph()` with `destroy_live_graph()`

### Command-Line Swift Specific Issues:
1. **"dylib not found" error**:
   ```bash
   # Check if dylib is in the right location
   ls -la audiograph/libaudiograph.dylib
   
   # Set library path explicitly
   export DYLD_LIBRARY_PATH="$PWD/audiograph:$DYLD_LIBRARY_PATH"
   swift run
   ```

2. **"module not found" error**:
   ```bash
   # Verify module.modulemap exists and is correct
   cat audiograph/module.modulemap
   
   # Check Package.swift systemLibrary path
   ```

3. **Linker errors during swift build**:
   ```bash
   # Try adding explicit linker flags
   swift build -Xlinker -L -Xlinker ./audiograph -Xlinker -laudiograph
   ```

4. **Runtime crashes with "image not found"**:
   ```bash
   # Check dylib architecture matches your system
   file audiograph/libaudiograph.dylib
   
   # Verify install name
   otool -D audiograph/libaudiograph.dylib
   ```

### Debug Build:
For debugging, use the debug version:
```bash
make clean && make lib debug
```

### Verbose Swift Build:
For troubleshooting build issues:
```bash
swift build --verbose
```

## OS Workgroup (Apple)
On iOS 15+/macOS 12+, co-schedule helper threads with the I/O thread by joining the Audio Unit's `os_workgroup_t`. This keeps multi-threaded processing aligned to the audio deadline.

1) Fetch the OS workgroup from the output AudioUnit and pass it to the engine:

```swift
import AVFAudio
import os

func bindOSWorkgroupFromAudioUnit(engine: AVAudioEngine) {
    guard let au: AudioUnit = engine.outputNode.audioUnit else { return }
    var wg: os_workgroup_t? = nil
    var size: UInt32 = UInt32(MemoryLayout<os_workgroup_t?>.size)
    let kAudioOutputUnitProperty_OSWorkgroup: AudioUnitPropertyID = 2015
    let status = AudioUnitGetProperty(
        au, kAudioOutputUnitProperty_OSWorkgroup,
        kAudioUnitScope_Global, 0,
        &wg, &size
    )
    if status == noErr, let wg {
        engine_set_os_workgroup(Unmanaged.passUnretained(wg).toOpaque())
    }
}
```

2) Optional (recommended): enable real-time time-constraint scheduling before starting workers:

```swift
engine_enable_rt_time_constraint(1)
engine_start_workers(2) // tune 2–3 based on graph width
```
