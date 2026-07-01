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
// Keyboard: CSS safe-area + visualViewport (--kb) in the web bundle.
#import <UIKit/UIKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

extern char *synapse_web_url(void);

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

@interface SynapseScriptHandler : NSObject <WKScriptMessageHandler>
@property (weak, nonatomic) WKWebView *webView;
@end

@implementation SynapseScriptHandler
- (void)userContentController:(WKUserContentController *)userContentController
      didReceiveScriptMessage:(WKScriptMessage *)message {
    if (![message.name isEqualToString:@"synapse"]) return;
    NSDictionary *body = [message.body isKindOfClass:[NSDictionary class]] ? message.body : nil;
    NSString *op = body[@"op"];
    if ([op isEqualToString:@"haptic"]) {
        NSString *style = body[@"style"] ?: @"light";
        UIImpactFeedbackStyle s = UIImpactFeedbackStyleLight;
        if ([style isEqualToString:@"medium"]) s = UIImpactFeedbackStyleMedium;
        else if ([style isEqualToString:@"heavy"]) s = UIImpactFeedbackStyleHeavy;
        UIImpactFeedbackGenerator *gen = [[UIImpactFeedbackGenerator alloc] initWithStyle:s];
        [gen prepare];
        [gen impactOccurred];
    } else if ([op isEqualToString:@"copy"]) {
        NSString *text = body[@"text"];
        if ([text isKindOfClass:[NSString class]] && text.length) {
            [UIPasteboard generalPasteboard].string = text;
        }
    } else if ([op isEqualToString:@"inputFocus"]) {
        if (self.webView) synapseRemoveInputAccessory(self.webView);
    }
}
@end

@interface SynapseWebDelegate : UIResponder <UIApplicationDelegate, WKNavigationDelegate>
@property (strong, nonatomic) UIWindow *window;
@property (strong, nonatomic) WKWebView *web;
@property (strong, nonatomic) SynapseScriptHandler *scriptHandler;
@end

@implementation SynapseWebDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    setenv("SYNAPSE_HOST", "127.0.0.1", 0);
    setenv("SYNAPSE_PORT", "4173", 0);
    setenv("SYNAPSE_TOKEN", "CODE", 0);

    self.window = [[UIWindow alloc] initWithFrame:[[UIScreen mainScreen] bounds]];

    WKWebViewConfiguration *cfg = [[WKWebViewConfiguration alloc] init];
    cfg.allowsInlineMediaPlayback = YES;
    self.scriptHandler = [[SynapseScriptHandler alloc] init];
    WKUserContentController *uc = [[WKUserContentController alloc] init];
    [uc addScriptMessageHandler:self.scriptHandler name:@"synapse"];
    NSString *bridgeJs =
        @"window.__synapseHaptic__=function(s){try{webkit.messageHandlers.synapse.postMessage({op:'haptic',style:s||'light'});}catch(e){}};"
         "window.__synapseCopy__=function(t){try{webkit.messageHandlers.synapse.postMessage({op:'copy',text:String(t||'')});}catch(e){}};"
         "document.addEventListener('focusin',function(e){if(e.target&&e.target.id==='input'){try{webkit.messageHandlers.synapse.postMessage({op:'inputFocus'});}catch(x){}}},true);";
    WKUserScript *script = [[WKUserScript alloc] initWithSource:bridgeJs
                                                  injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                               forMainFrameOnly:YES];
    [uc addUserScript:script];
    cfg.userContentController = uc;

    self.web = [[WKWebView alloc] initWithFrame:self.window.bounds configuration:cfg];
    self.scriptHandler.webView = self.web;
    self.web.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    self.web.navigationDelegate = self;
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
        [self.web loadHTMLString:@"<h2 style='font-family:sans-serif;padding:40px'>Could not start chat host.</h2>"
                         baseURL:nil];
    }
    return YES;
}

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
