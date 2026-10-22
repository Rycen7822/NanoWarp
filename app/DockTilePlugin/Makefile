BUNDLE_NAME = WarpDockTilePlugin.docktileplugin
OUTPUT_DIR ?= .
OBJC_FILES = WarpDockTilePlugin.m
FRAMEWORKS = -framework Cocoa -framework AppKit -framework Foundation
# Compile a universal binary for both arm64 and x86_64.
CFLAGS = -fobjc-arc -bundle -mmacosx-version-min=$(MACOSX_DEPLOYMENT_TARGET) -arch arm64 -arch x86_64
LDFLAGS = -ObjC

.PHONY: all clean

all: $(BUNDLE_NAME)

$(BUNDLE_NAME): $(OBJC_FILES)
	mkdir -p "$(OUTPUT_DIR)/$(BUNDLE_NAME)/Contents/MacOS"
	clang $(CFLAGS) $(LDFLAGS) $(FRAMEWORKS) $(OBJC_FILES) -o "$(OUTPUT_DIR)/$(BUNDLE_NAME)/Contents/MacOS/WarpDockTilePlugin"
	cp Info.plist "$(OUTPUT_DIR)/$(BUNDLE_NAME)/Contents/"
	mkdir -p "$(OUTPUT_DIR)/$(BUNDLE_NAME)/Contents/Resources"
	cp Resources/* "$(OUTPUT_DIR)/$(BUNDLE_NAME)/Contents/Resources/"

clean:
	rm -rf "$(OUTPUT_DIR)/$(BUNDLE_NAME)"
