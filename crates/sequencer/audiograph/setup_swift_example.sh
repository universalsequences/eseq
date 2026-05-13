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
echo ""
echo "To run tests on the library itself:"
echo "  make test                     # Run all tests"
echo "  ./tests/test_sum_behavior     # Test specific functionality"