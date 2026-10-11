/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSURLConnection`.

use super::{ns_string, NSInteger};
use crate::environment::Environment;
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use std::borrow::Cow;

const NSURLErrorDomain: &str = "NSURLErrorDomain";

/// Our helper type, Foundation just uses ints.
type NSURLErrorCode = NSInteger;
const NSURLErrorNotConnectedToInternet: NSURLErrorCode = -1009;

#[derive(Default)]
struct ConnectionHostObject {
    delegate: id,
    started: bool,
    finished: bool,
}
impl HostObject for ConnectionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURLConnection: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<ConnectionHostObject>::default(), &mut env.mem)
}

+ (id)sendSynchronousRequest:(id)request // NSURLRequest *
           returningResponse:(MutPtr<id>)response // NSURLResponse **
                       error:(MutPtr<id>)out_error { // NSError **
    log!(
        "TODO: [NSURLConnection sendSynchronousRequest:{:?} ('{}') response:{:?} error:{:?}] -> nil",
        request,
        url_string_from_request(env, request),
        response,
        out_error,
    );
    if !response.is_null() {
        env.mem.write(response, nil);
    }
    if !out_error.is_null() {
        let domain = ns_string::get_static_str(env, NSURLErrorDomain);
        let error = msg_class![env; NSError alloc];
        // TODO: fill userInfo
        let error = msg![env; error initWithDomain:domain code:NSURLErrorNotConnectedToInternet userInfo:nil];
        autorelease(env, error);
        env.mem.write(out_error, error);
    }
    nil
}

+ (id)connectionWithRequest:(id)request // NSURLRequest *
                   delegate:(id)delegate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithRequest:request delegate:delegate];
    autorelease(env, new)
}

- (id)initWithRequest:(id)request // NSURLRequest *
             delegate:(id)delegate {
    msg![env; this initWithRequest:request delegate:delegate startImmediately:true]
}

- (id)initWithRequest:(id)request // NSURLRequest *
             delegate:(id)delegate
     startImmediately:(bool)start_immediately {
    log!(
        "TODO: [(NSURLConnection *){:?} initWithRequest:{:?} ('{}') delegate:{:?} startImmediately:{}]: reporting offline",
        this,
        request,
        url_string_from_request(env, request),
        delegate,
        start_immediately,
    );
    if request == nil {
        release(env, this);
        return nil;
    }
    retain(env, delegate);
    env.objc.borrow_mut::<ConnectionHostObject>(this).delegate = delegate;
    if start_immediately {
        () = msg![env; this start];
    }
    this
}

- (())start {
    let host = env.objc.borrow_mut::<ConnectionHostObject>(this);
    if host.started || host.finished {
        return;
    }
    host.started = true;
    let selector = env.objc.lookup_selector("_touchHLE_connectionFailed:").unwrap();
    () = msg![env; this performSelector:selector withObject:nil afterDelay:0.0f64];
}

- (())_touchHLE_connectionFailed:(id)_unused {
    let host = env.objc.borrow_mut::<ConnectionHostObject>(this);
    if host.finished {
        return;
    }
    host.finished = true;
    let delegate = std::mem::replace(&mut host.delegate, nil);
    let selector = env.objc.register_host_selector("connection:didFailWithError:".to_owned(), &mut env.mem);
    if msg![env; delegate respondsToSelector:selector] {
        let domain = ns_string::get_static_str(env, NSURLErrorDomain);
        let error: id = msg_class![env; NSError errorWithDomain:domain code:NSURLErrorNotConnectedToInternet userInfo:nil];
        () = msg![env; delegate connection:this didFailWithError:error];
    }
    release(env, delegate);
}

- (())cancel {
    let host = env.objc.borrow_mut::<ConnectionHostObject>(this);
    host.finished = true;
    let delegate = std::mem::replace(&mut host.delegate, nil);
    release(env, delegate);
}

- (())dealloc {
    let delegate = env.objc.borrow::<ConnectionHostObject>(this).delegate;
    release(env, delegate);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

fn url_string_from_request(env: &mut Environment, request: id) -> Cow<'static, str> {
    if request == nil {
        Cow::from("(null)")
    } else {
        let url = msg![env; request URL];
        let ns_string = msg![env; url absoluteString];
        ns_string::to_rust_string(env, ns_string)
    }
}
