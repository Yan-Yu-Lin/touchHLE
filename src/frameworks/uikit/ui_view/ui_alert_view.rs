/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAlertView`.
//! TODO: animations, automatic text wrapping, and updating visible alerts.

use super::ui_control::{UIControlEventTouchUpInside, UIControlStateNormal};
use super::UIViewHostObject;
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::frameworks::uikit::ui_font::UITextAlignmentCenter;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_send, msg_super, nil, objc_classes,
    release, retain, ClassExports, NSZonePtr,
};
use crate::Environment;

struct UIAlertViewHostObject {
    superclass: UIViewHostObject,
    title: id,
    message: id,
    delegate: id, // Weak reference.
    buttons: Vec<id>,
    cancel_index: NSInteger,
    window: id,
    previous_window: id, // Weak reference.
    decorated: bool,
}
impl_HostObject_with_superclass!(UIAlertViewHostObject);
impl Default for UIAlertViewHostObject {
    fn default() -> Self {
        Self {
            superclass: Default::default(),
            title: nil,
            message: nil,
            delegate: nil,
            buttons: Vec::new(),
            cancel_index: -1,
            window: nil,
            previous_window: nil,
            decorated: false,
        }
    }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

fn notify(env: &mut Environment, this: id, name: &str, index: Option<NSInteger>) {
    let delegate = env.objc.borrow::<UIAlertViewHostObject>(this).delegate;
    let selector = env
        .objc
        .register_host_selector(name.to_owned(), &mut env.mem);
    if msg![env; delegate respondsToSelector:selector] {
        if let Some(index) = index {
            () = msg_send(env, (delegate, selector, this, index));
        } else {
            () = msg_send(env, (delegate, selector, this));
        }
    }
}

fn add_label(env: &mut Environment, this: id, text: id, frame: CGRect, size: f32) {
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    let color: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:color];
    let color: id = msg_class![env; UIColor whiteColor];
    () = msg![env; label setTextColor:color];
    let font: id = msg_class![env; UIFont systemFontOfSize:size];
    () = msg![env; label setFont:font];
    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    () = msg![env; label setNumberOfLines:0i32];
    () = msg![env; label setText:text];
    // Keep application-provided controls above the labels.
    () = msg![env; this insertSubview:label atIndex:0i32];
    release(env, label);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIAlertView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIAlertViewHostObject>::default(), &mut env.mem)
}

- (id)initWithTitle:(id)title
            message:(id)message
           delegate:(id)delegate
  cancelButtonTitle:(id)cancel_title
  otherButtonTitles:(id)other_title, ...args {
    let this: id = msg_super![env; this init];
    () = msg![env; this setTitle:title];
    () = msg![env; this setMessage:message];
    () = msg![env; this setDelegate:delegate];
    if cancel_title != nil {
        let index: NSInteger = msg![env; this addButtonWithTitle:cancel_title];
        env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_index = index;
    }
    if other_title != nil {
        let _: NSInteger = msg![env; this addButtonWithTitle:other_title];
        let mut args = args.start();
        loop {
            let title: id = args.next(env);
            if title == nil {
                break;
            }
            let _: NSInteger = msg![env; this addButtonWithTitle:title];
        }
    }
    this
}

- (id)title { env.objc.borrow::<UIAlertViewHostObject>(this).title }
- (())setTitle:(id)title {
    retain(env, title);
    let old = std::mem::replace(&mut env.objc.borrow_mut::<UIAlertViewHostObject>(this).title, title);
    release(env, old);
}
- (id)message { env.objc.borrow::<UIAlertViewHostObject>(this).message }
- (())setMessage:(id)message {
    retain(env, message);
    let old = std::mem::replace(&mut env.objc.borrow_mut::<UIAlertViewHostObject>(this).message, message);
    release(env, old);
}
- (id)delegate { env.objc.borrow::<UIAlertViewHostObject>(this).delegate }
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).delegate = delegate;
}
- (bool)isVisible { env.objc.borrow::<UIAlertViewHostObject>(this).window != nil }

- (NSInteger)addButtonWithTitle:(id)title {
    retain(env, title);
    let buttons = &mut env.objc.borrow_mut::<UIAlertViewHostObject>(this).buttons;
    let index = buttons.len() as NSInteger;
    buttons.push(title);
    index
}
- (NSInteger)numberOfButtons {
    env.objc.borrow::<UIAlertViewHostObject>(this).buttons.len() as NSInteger
}
- (id)buttonTitleAtIndex:(NSInteger)index {
    env.objc.borrow::<UIAlertViewHostObject>(this).buttons[index as usize]
}
- (NSInteger)cancelButtonIndex {
    env.objc.borrow::<UIAlertViewHostObject>(this).cancel_index
}
- (())setCancelButtonIndex:(NSInteger)index {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_index = index;
}
- (NSInteger)firstOtherButtonIndex {
    let host = env.objc.borrow::<UIAlertViewHostObject>(this);
    (0..host.buttons.len() as NSInteger).find(|&i| i != host.cancel_index).unwrap_or(-1)
}

- (())show {
    if msg![env; this isVisible] {
        return;
    }
    let host = env.objc.borrow::<UIAlertViewHostObject>(this);
    let (title, message, buttons, decorated) = (host.title, host.message, host.buttons.clone(), host.decorated);
    let lines = if message == nil { 0 } else { ns_string::to_rust_string(env, message).split('\n').count() };
    let button_y = 50.0 + lines as f32 * 20.0;
    let rows = if buttons.len() <= 2 { 1 } else { buttons.len() };
    let height = button_y + rows as f32 * 44.0 + 10.0;
    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];
    let frame = rect((bounds.size.width - 284.0) / 2.0, (bounds.size.height - height) / 2.0, 284.0, height);
    log_dbg!("Showing alert {:?}, frame {:?}, title {:?}", this, frame, title);
    () = msg![env; this setFrame:frame];
    let background: id = msg_class![env; UIColor colorWithRed:0.15f32 green:0.18f32 blue:0.25f32 alpha:1.0f32];
    () = msg![env; this setBackgroundColor:background];
    if !decorated {
        add_label(env, this, title, rect(10.0, 10.0, 264.0, 30.0), 20.0);
        add_label(env, this, message, rect(10.0, 45.0, 264.0, lines as f32 * 20.0), 16.0);
        let action = env.objc.lookup_selector("_touchHLE_alertButtonTapped:").unwrap();
        for (index, title) in buttons.into_iter().enumerate() {
            let count: NSInteger = msg![env; this numberOfButtons];
            let (x, y, width) = if count <= 2 {
                let width = 264.0 / count as f32;
                (10.0 + index as f32 * width, button_y, width - 4.0)
            } else {
                (10.0, button_y + index as f32 * 44.0, 264.0)
            };
            let button: id = msg_class![env; UIButton buttonWithType:0i32];
            () = msg![env; button setFrame:(rect(x, y, width, 40.0))];
            let color: id = msg_class![env; UIColor grayColor];
            () = msg![env; button setBackgroundColor:color];
            () = msg![env; button setTitle:title forState:UIControlStateNormal];
            log_dbg!("Button {}: {:?}", index, title);
            () = msg![env; button setTag:(index as NSInteger)];
            () = msg![env; button addTarget:this action:action forControlEvents:UIControlEventTouchUpInside];
            () = msg![env; this addSubview:button];
        }
        env.objc.borrow_mut::<UIAlertViewHostObject>(this).decorated = true;
    }
    let previous: id = msg_class![env; UIApplication sharedApplication];
    let previous: id = msg![env; previous keyWindow];
    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];
    let background: id = msg_class![env; UIColor colorWithWhite:0.0f32 alpha:0.5f32];
    () = msg![env; window setBackgroundColor:background];
    let host = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
    host.window = window;
    host.previous_window = previous;
    notify(env, this, "willPresentAlertView:", None);
    () = msg![env; window addSubview:this];
    () = msg![env; window makeKeyAndVisible];
    notify(env, this, "didPresentAlertView:", None);
}

- (())_touchHLE_alertButtonTapped:(id)sender {
    // A callback may release the application's last reference to the alert.
    retain(env, this);
    let index: NSInteger = msg![env; sender tag];
    log_dbg!("Alert {:?} clicked button {}", this, index);
    notify(env, this, "alertView:clickedButtonAtIndex:", Some(index));
    () = msg![env; this dismissWithClickedButtonIndex:index animated:true];
    release(env, this);
}

- (())dismissWithClickedButtonIndex:(NSInteger)index animated:(bool)_animated {
    let host = env.objc.borrow::<UIAlertViewHostObject>(this);
    let (window, previous) = (host.window, host.previous_window);
    if window == nil {
        return;
    }
    retain(env, this);
    notify(env, this, "alertView:willDismissWithButtonIndex:", Some(index));
    let responder = env.framework_state.uikit.ui_responder.first_responder;
    if responder != nil {
        let owner: id = msg![env; responder window];
        if owner == window {
            let _: bool = msg![env; responder resignFirstResponder];
        }
    }
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).window = nil;
    () = msg![env; this removeFromSuperview];
    () = msg![env; window setHidden:true];
    let is_key: bool = msg![env; window isKeyWindow];
    if is_key && previous != nil {
        () = msg![env; previous makeKeyWindow];
    }
    release(env, window);
    notify(env, this, "alertView:didDismissWithButtonIndex:", Some(index));
    release(env, this);
}

- (())dealloc {
    let host = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
    let (title, message, buttons) = (host.title, host.message, std::mem::take(&mut host.buttons));
    release(env, title);
    release(env, message);
    for title in buttons {
        release(env, title);
    }
    msg_super![env; this dealloc]
}

@end

};
