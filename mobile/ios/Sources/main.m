// iOS app entry — WKWebView chat host.
//
// The post-pairing chat experience is a web app (crates/app/web) served from
// an embedded localhost host inside the Rust library. This entry boots a
// standard UIKit app whose root view controller hosts a full-screen WKWebView
// pointed at that host. The web app is a WS client that dials the Synapse
// server itself; credentials ride in the URL query (host/port/token/tls).
//
// Why a plain UIKit app (not Slint-on-winit here): the chat UI no longer needs
// Slint, and a WKWebView as the app's root view is the robust standard pattern.
// On-device pairing is currently env-var auto-connect (see synapse_ios_main in
// lib.rs); a real pairing screen is a documented later item.
//
// The keyboard-frame observer is gone: WKWebView + viewport-fit=cover and the
// CSS safe-area insets handle the on-screen keyboard natively.
#import <UIKit/UIKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

// Starts the embedded web host and returns the URL to load (malloc'd C string,
// caller frees). Null on failure. Exported from crates/app/src/lib.rs.
extern char *synapse_web_url(void);
// Reads SYNAPSE_HOST/PORT/TOKEN before the host starts. Exported from lib.rs;
// here we just rely on the lib's own defaults set in synapse_ios_main path.
// (The web URL function reads the environment itself.)

// Remove the keyboard's input-accessory bar (the up/down/Done navigation strip
// iOS injects above the keyboard for web form fields). There is no public API
// to disable it on WKWebView, so we dynamically subclass the internal
// WKContentView and override -inputAccessoryView to return nil. This is the
// standard, widely-used workaround.
// ponytail: runtime-subclass hack; the ceiling is a future iOS renaming
// WKContentView — guarded by the prefix check, which simply no-ops if so.
static void synapseRemoveInputAccessory(WKWebView *webView) {
    UIView *contentView = nil;
    for (UIView *v in webView.scrollView.subviews) {
        if ([NSStringFromClass(v.class) hasPrefix:@"WKContent"]) {
            contentView = v;
            break;
        }
    }
    if (!contentView) return;
    NSString *subName = [NSStringFromClass(contentView.class) stringByAppendingString:@"_NoAccessory"];
    Class subclass = NSClassFromString(subName);
    if (!subclass) {
        subclass = objc_allocateClassPair(contentView.class, subName.UTF8String, 0);
        if (!subclass) return;
        IMP nilImp = imp_implementationWithBlock(^id(id _self) { return nil; });
        class_addMethod(subclass, @selector(inputAccessoryView), nilImp, "@@:");
        objc_registerClassPair(subclass);
    }
    object_setClass(contentView, subclass);
}

@interface SynapseWebDelegate : UIResponder <UIApplicationDelegate, WKNavigationDelegate>
@property (strong, nonatomic) UIWindow *window;
@property (strong, nonatomic) WKWebView *web;
@end

@implementation SynapseWebDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    // Default connection env for simulator/dev. A pairing flow would set these
    // before the web URL is built.
    setenv("SYNAPSE_HOST", "127.0.0.1", 0);
    setenv("SYNAPSE_PORT", "4173", 0);
    setenv("SYNAPSE_TOKEN", "CODE", 0);

    self.window = [[UIWindow alloc] initWithFrame:[[UIScreen mainScreen] bounds]];

    WKWebViewConfiguration *cfg = [[WKWebViewConfiguration alloc] init];
    cfg.allowsInlineMediaPlayback = YES;
    self.web = [[WKWebView alloc] initWithFrame:self.window.bounds configuration:cfg];
    self.web.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    self.web.navigationDelegate = self;
    // Let CSS env(safe-area-inset-*) drive insets; webview fills the window.
    self.web.scrollView.contentInsetAdjustmentBehavior = UIScrollViewContentInsetAdjustmentNever;

    UIViewController *vc = [[UIViewController alloc] init];
    [vc.view addSubview:self.web];
    self.web.frame = vc.view.bounds;
    self.window.rootViewController = vc;
    [self.window makeKeyAndVisible];

    char *curl = synapse_web_url();
    if (curl) {
        NSString *urlStr = [NSString stringWithUTF8String:curl];
        free(curl);
        NSURL *url = [NSURL URLWithString:urlStr];
        if (url) {
            [self.web loadRequest:[NSURLRequest requestWithURL:url]];
        }
    } else {
        // Host failed to start — show a minimal message.
        [self.web loadHTMLString:@"<h2 style='font-family:sans-serif;padding:40px'>Could not start chat host.</h2>"
                         baseURL:nil];
    }
    return YES;
}

// Strip the keyboard accessory bar once the content view exists (post-load).
- (void)webView:(WKWebView *)webView didFinishNavigation:(WKNavigation *)navigation {
    synapseRemoveInputAccessory(webView);
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil,
                                 NSStringFromClass([SynapseWebDelegate class]));
    }
}
