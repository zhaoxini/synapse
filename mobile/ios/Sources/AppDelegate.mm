#import "AppDelegate.h"

// NOTE: This class is retained for reference but is no longer the app's entry
// point. The active entry is mobile/ios/Sources/main.m, which calls
// synapse_ios_main() directly. Slint's winit backend calls UIApplicationMain()
// itself (from App::run() -> EventLoop::run()), creates its own UIWindow, and
// registers UIKit observers via objc2 — so no ObjC UIApplicationDelegate is
// needed to boot the UI. Keeping this file linked is harmless; the class simply
// is never instantiated.

@implementation SynapseAppDelegate
@end
