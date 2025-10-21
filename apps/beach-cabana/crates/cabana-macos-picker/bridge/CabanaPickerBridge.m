#import "CabanaPickerBridge.h"

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreFoundation/CoreFoundation.h>
#import <dispatch/dispatch.h>
#import <os/log.h>
#import <os/object.h>
#import <string.h>

@class CabanaPickerController;

typedef struct {
    CabanaPickerEventCallback callback;
    void *userData;
    BOOL observing;
} CabanaPickerState;

typedef struct {
    CabanaPickerState *state;
    CFTypeRef controller_ref; // Retained CabanaPickerController*
} CabanaPickerWrapper;

static inline CabanaPickerWrapper *CabanaUnwrap(void *handle) {
    return (CabanaPickerWrapper *)handle;
}

static inline CabanaPickerController *CabanaUnwrapController(CabanaPickerWrapper *wrapper) {
    if (!wrapper || !wrapper->controller_ref) {
        return nil;
    }
    return (__bridge CabanaPickerController *)wrapper->controller_ref;
}

static const char *CabanaCreateCStringFromNSString(NSString *string) {
    if (string == nil) {
        return NULL;
    }
    const char *utf8 = [string UTF8String];
    if (utf8 == NULL) {
        return NULL;
    }
    char *copied = strdup(utf8);
    return copied;
}

static NSDictionary *CabanaCreateSelectionPayload(SCContentFilter *filter, SCStream *stream);
static NSString *CabanaDisplayNameForDisplay(SCDisplay *display);
static void CabanaEmitDictionary(CabanaPickerState *state, dispatch_queue_t queue, NSDictionary *dictionary);
static void CabanaEmitSimpleEvent(CabanaPickerState *state, dispatch_queue_t queue, NSString *type, NSString *message);

@interface CabanaPickerController : NSObject <SCContentSharingPickerObserver>
@property (nonatomic, readonly) dispatch_queue_t eventQueue;
- (instancetype)initWithState:(CabanaPickerState *)state;
- (BOOL)presentWithError:(NSError **)error;
- (void)cancelPresentation;
- (void)invalidate;
@end

@implementation CabanaPickerController {
    CabanaPickerState *_state;
    dispatch_queue_t _eventQueue;
}

- (instancetype)initWithState:(CabanaPickerState *)state {
    self = [super init];
    if (self) {
        _state = state;
        _eventQueue = dispatch_queue_create("com.beach.cabana.picker.events", DISPATCH_QUEUE_SERIAL);
    }
    return self;
}

- (void)dealloc {
    [self invalidate];
}

- (dispatch_queue_t)eventQueue {
    return _eventQueue;
}

- (BOOL)presentWithError:(NSError **)error {
    if (@available(macOS 14.0, *)) {
        dispatch_async(dispatch_get_main_queue(), ^{
            SCContentSharingPicker *picker = [SCContentSharingPicker sharedPicker];
            if (!picker) {
                CabanaEmitSimpleEvent(self->_state, self->_eventQueue, @"error", @"Native picker unavailable");
                return;
            }

            if (!self->_state->observing) {
                [picker addObserver:self];
                self->_state->observing = YES;
            }

            SCContentSharingPickerConfiguration *configuration = [picker.defaultConfiguration copy] ?: [[SCContentSharingPickerConfiguration alloc] init];
            configuration.allowedPickerModes =
                SCContentSharingPickerModeSingleWindow |
                SCContentSharingPickerModeMultipleWindows |
                SCContentSharingPickerModeSingleApplication |
                SCContentSharingPickerModeSingleDisplay;
            picker.defaultConfiguration = configuration;
            picker.maximumStreamCount = @(1);
            picker.active = YES;
            [picker present];
        });
        return YES;
    } else {
        if (error) {
            *error = [NSError errorWithDomain:@"cabana.picker" code:-1 userInfo:@{NSLocalizedDescriptionKey: @"SCContentSharingPicker requires macOS 14"}];
        }
        return NO;
    }
}

- (void)cancelPresentation {
    if (@available(macOS 14.0, *)) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!self->_state->observing) {
                return;
            }
            SCContentSharingPicker *picker = [SCContentSharingPicker sharedPicker];
            [picker removeObserver:self];
            self->_state->observing = NO;
            picker.active = NO;
        });
    }
}

- (void)invalidate {
    if (!self->_state) {
        return;
    }
    if (@available(macOS 14.0, *)) {
        dispatch_sync(dispatch_get_main_queue(), ^{
            if (!self->_state->observing) {
                return;
            }
            SCContentSharingPicker *picker = [SCContentSharingPicker sharedPicker];
            [picker removeObserver:self];
            self->_state->observing = NO;
            picker.active = NO;
        });
    }
    if (_eventQueue) {
        dispatch_sync(_eventQueue, ^{});
#if !OS_OBJECT_USE_OBJC
        dispatch_release(_eventQueue);
#endif
        _eventQueue = NULL;
    }
    _state = NULL;
}

- (void)contentSharingPicker:(SCContentSharingPicker *)picker
           didUpdateWithFilter:(SCContentFilter *)filter
                     forStream:(SCStream *)stream API_AVAILABLE(macos(14.0)) {
    if (!_state || !filter) {
        return;
    }
    NSDictionary *payload = CabanaCreateSelectionPayload(filter, stream);
    CabanaEmitDictionary(_state, _eventQueue, payload);
}

- (void)contentSharingPicker:(SCContentSharingPicker *)picker
           didCancelForStream:(SCStream *)stream API_AVAILABLE(macos(14.0)) {
    if (!_state) {
        return;
    }
    CabanaEmitSimpleEvent(_state, _eventQueue, @"cancelled", nil);
}

- (void)contentSharingPickerStartDidFailWithError:(NSError *)error API_AVAILABLE(macos(14.0)) {
    if (!_state) {
        return;
    }
    NSString *message = error.localizedDescription ?: @"Unknown error";
    CabanaEmitSimpleEvent(_state, _eventQueue, @"error", message);
}

@end

bool cabana_picker_is_available(void) {
    if (@available(macOS 14.0, *)) {
        return [SCContentSharingPicker respondsToSelector:@selector(sharedPicker)];
    }
    return false;
}

static void CabanaFillError(const char **error_message, NSString *message) {
    if (!error_message) {
        return;
    }
    const char *copy = CabanaCreateCStringFromNSString(message);
    *error_message = copy;
}

void *cabana_picker_create(CabanaPickerEventCallback callback,
                           void *user_data,
                           const char **error_message) {
    if (!cabana_picker_is_available()) {
        CabanaFillError(error_message, @"SCContentSharingPicker requires macOS 14 or later");
        return NULL;
    }

    CabanaPickerState *state = calloc(1, sizeof(CabanaPickerState));
    if (!state) {
        CabanaFillError(error_message, @"Failed to allocate picker state");
        return NULL;
    }

    state->callback = callback;
    state->userData = user_data;
    state->observing = NO;

    CabanaPickerController *controller = [[CabanaPickerController alloc] initWithState:state];
    if (!controller) {
        CabanaFillError(error_message, @"Failed to initialize picker controller");
        free(state);
        return NULL;
    }

    CabanaPickerWrapper *wrapper = calloc(1, sizeof(CabanaPickerWrapper));
    if (!wrapper) {
        CabanaFillError(error_message, @"Failed to allocate picker wrapper");
        free(state);
        return NULL;
    }

    wrapper->state = state;
    wrapper->controller_ref = (__bridge_retained CFTypeRef)controller;
    return wrapper;
}

bool cabana_picker_present(void *handle, const char **error_message) {
    CabanaPickerWrapper *wrapper = CabanaUnwrap(handle);
    if (!wrapper) {
        CabanaFillError(error_message, @"picker handle was null");
        return false;
    }

    CabanaPickerController *controller = CabanaUnwrapController(wrapper);
    if (!controller) {
        CabanaFillError(error_message, @"picker controller unavailable");
        return false;
    }
    NSError *error = nil;
    BOOL ok = [controller presentWithError:&error];

    if (!ok && error) {
        CabanaFillError(error_message, error.localizedDescription ?: @"Failed to present picker");
    }
    return ok;
}

void cabana_picker_cancel(void *handle) {
    CabanaPickerWrapper *wrapper = CabanaUnwrap(handle);
    if (!wrapper) {
        return;
    }
    CabanaPickerController *controller = CabanaUnwrapController(wrapper);
    [controller cancelPresentation];
}

void cabana_picker_destroy(void *handle) {
    CabanaPickerWrapper *wrapper = CabanaUnwrap(handle);
    if (!wrapper) {
        return;
    }
    CabanaPickerController *controller = nil;
    if (wrapper->controller_ref) {
        controller = (__bridge_transfer CabanaPickerController *)wrapper->controller_ref;
        wrapper->controller_ref = NULL;
    }
    if (controller) {
        dispatch_queue_t queue = controller.eventQueue;
        if (queue) {
            dispatch_sync(queue, ^{});
        }
        [controller invalidate];
    }

    CabanaPickerState *state = wrapper->state;
    if (state) {
        free(state);
        wrapper->state = NULL;
    }

    free(wrapper);
}

void cabana_picker_free_c_string(const char *ptr) {
    if (ptr) {
        free((void *)ptr);
    }
}

static void CabanaEmitDictionary(CabanaPickerState *state, dispatch_queue_t queue, NSDictionary *dictionary) {
    if (!state || !state->callback || !dictionary || !queue) {
        return;
    }

    dispatch_async(queue, ^{
        @autoreleasepool {
            NSError *error = nil;
            NSData *jsonData = [NSJSONSerialization dataWithJSONObject:dictionary options:0 error:&error];
            if (!jsonData) {
                os_log_error(OS_LOG_DEFAULT, "Cabana picker failed to serialize event: %{public}@", error.localizedDescription);
                return;
            }

            NSString *jsonString = [[NSString alloc] initWithData:jsonData encoding:NSUTF8StringEncoding];
            if (!jsonString) {
                os_log_error(OS_LOG_DEFAULT, "Cabana picker produced invalid UTF-8 event");
                return;
            }

            const char *utf8 = [jsonString UTF8String];
            if (!utf8) {
                return;
            }

            char *copy = strdup(utf8);
            if (!copy) {
                return;
            }

            state->callback(copy, state->userData);
            free(copy);
        }
    });
}

static void CabanaEmitSimpleEvent(CabanaPickerState *state, dispatch_queue_t queue, NSString *type, NSString *message) {
    NSMutableDictionary *dictionary = [NSMutableDictionary dictionary];
    dictionary[@"type"] = type ?: @"event";
    if (message) {
        dictionary[@"message"] = message;
    }
    CabanaEmitDictionary(state, queue, dictionary);
}

static NSDictionary *CabanaCreateSelectionPayload(SCContentFilter *filter, SCStream *stream) API_AVAILABLE(macos(14.0)) {
    (void)stream;
    NSMutableDictionary *payload = [NSMutableDictionary dictionary];
    payload[@"type"] = @"selection";

    NSMutableDictionary *metadata = [NSMutableDictionary dictionary];

    NSString *kind = @"unknown";
    NSString *identifier = nil;
    NSString *label = nil;
    NSString *application = nil;

    NSArray<SCDisplay *> *displays = nil;
    if (@available(macOS 15.2, *)) {
        displays = filter.includedDisplays;
    }
    if (displays.count > 0) {
        SCDisplay *display = displays.firstObject;
        identifier = [NSString stringWithFormat:@"display:%u", display.displayID];
        label = CabanaDisplayNameForDisplay(display);
        kind = @"display";
        metadata[@"display_id"] = @(display.displayID);
        CGRect frame = display.frame;
        metadata[@"width"] = @(CGRectGetWidth(frame));
        metadata[@"height"] = @(CGRectGetHeight(frame));
    } else {
        NSArray<SCWindow *> *windows = nil;
        if (@available(macOS 15.2, *)) {
            windows = filter.includedWindows;
        }
        if (windows.count > 0) {
            SCWindow *window = windows.firstObject;
            identifier = [NSString stringWithFormat:@"window:%u", window.windowID];
            label = window.title ?: @"Untitled Window";
            kind = @"window";
            metadata[@"window_id"] = @(window.windowID);
            metadata[@"layer"] = @(window.windowLayer);
            metadata[@"is_on_screen"] = @(window.isOnScreen);
            CGRect frame = window.frame;
            metadata[@"width"] = @(CGRectGetWidth(frame));
            metadata[@"height"] = @(CGRectGetHeight(frame));

            SCRunningApplication *app = window.owningApplication;
            if (app) {
                application = app.applicationName;
                metadata[@"bundle_id"] = app.bundleIdentifier ?: @"";
                metadata[@"process_id"] = @(app.processID);
            }
        } else {
            NSArray<SCRunningApplication *> *applications = nil;
            if (@available(macOS 15.2, *)) {
                applications = filter.includedApplications;
            }
            if (applications.count > 0) {
                SCRunningApplication *app = applications.firstObject;
                identifier = [NSString stringWithFormat:@"application:%@", app.bundleIdentifier ?: @"unknown"];
                application = app.applicationName;
                label = app.applicationName;
                kind = @"application";
                metadata[@"bundle_id"] = app.bundleIdentifier ?: @"";
                metadata[@"process_id"] = @(app.processID);
            } else {
                SCShareableContentStyle style = SCShareableContentStyleNone;
                if (@available(macOS 14.0, *)) {
                    style = filter.style;
                }
                switch (style) {
                    case SCShareableContentStyleDisplay:
                        kind = @"display";
                        label = @"Selected Display";
                        break;
                    case SCShareableContentStyleWindow:
                        kind = @"window";
                        label = @"Selected Window";
                        break;
                    case SCShareableContentStyleApplication:
                        kind = @"application";
                        label = @"Selected Application";
                        break;
                    default:
                        kind = @"unknown";
                        label = @"Selected Content";
                        break;
                }
            }
        }
    }

    if (@available(macOS 14.0, *)) {
        SCShareableContentInfo *info = [SCShareableContent infoForFilter:filter];
        if (info) {
            CGRect rect = info.contentRect;
            if (!metadata[@"width"]) {
                metadata[@"width"] = @(CGRectGetWidth(rect));
            }
            if (!metadata[@"height"]) {
                metadata[@"height"] = @(CGRectGetHeight(rect));
            }
        }
    }

    if (!label) {
        label = @"Selection";
    }
    if (!identifier) {
        identifier = [[NSUUID UUID] UUIDString];
    }

    payload[@"id"] = identifier;
    payload[@"label"] = label;
    payload[@"kind"] = kind;
    if (application) {
        payload[@"application"] = application;
    }

    NSError *archiveError = nil;
    NSData *filterData = [NSKeyedArchiver archivedDataWithRootObject:filter requiringSecureCoding:YES error:&archiveError];
    if (filterData) {
        payload[@"filter"] = [filterData base64EncodedStringWithOptions:0];
    } else if (archiveError) {
        os_log_error(OS_LOG_DEFAULT, "Cabana picker failed to encode filter: %{public}@", archiveError.localizedDescription);
    }

    if (metadata.count > 0) {
        payload[@"metadata"] = metadata;
    }

    return payload;
}

static NSString *CabanaDisplayNameForDisplay(SCDisplay *display) API_AVAILABLE(macos(14.0)) {
    if (@available(macOS 10.15, *)) {
        for (NSScreen *screen in [NSScreen screens]) {
            NSNumber *screenNumber = screen.deviceDescription[@"NSScreenNumber"];
            if (screenNumber && screenNumber.unsignedIntValue == display.displayID) {
                if ([screen respondsToSelector:@selector(localizedName)]) {
                    NSString *localized = screen.localizedName;
                    if (localized.length > 0) {
                        return localized;
                    }
                }
            }
        }
    }
    return [NSString stringWithFormat:@"Display %u", display.displayID];
}
