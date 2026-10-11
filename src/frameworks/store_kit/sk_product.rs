/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{
    id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

#[derive(Default)]
struct RequestHostObject {
    delegate: id, // Non-retaining.
    identifiers: id,
    started: bool,
    cancelled: bool,
}
impl HostObject for RequestHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKProduct: NSObject
// TODO
@end

@implementation SKRequest: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<RequestHostObject>::default(), &mut env.mem)
}
- (id)delegate { env.objc.borrow::<RequestHostObject>(this).delegate }
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<RequestHostObject>(this).delegate = delegate;
}
- (())start {
    let host = env.objc.borrow_mut::<RequestHostObject>(this);
    if host.started || host.cancelled { return; }
    host.started = true;
    let selector = env.objc.lookup_selector("_touchHLE_requestFailed:").unwrap();
    () = msg![env; this performSelector:selector withObject:nil afterDelay:0.0f64];
}
- (())cancel { env.objc.borrow_mut::<RequestHostObject>(this).cancelled = true; }
- (())_touchHLE_requestFailed:(id)_unused {
    let host = env.objc.borrow::<RequestHostObject>(this);
    if host.cancelled { return; }
    let delegate = host.delegate;
    retain(env, delegate);
    let selector = env.objc.register_host_selector("request:didFailWithError:".to_owned(), &mut env.mem);
    if msg![env; delegate respondsToSelector:selector] {
        let error = super::unavailable_error(env);
        () = msg![env; delegate request:this didFailWithError:error];
    }
    release(env, delegate);
}
- (())dealloc {
    let ids = env.objc.borrow::<RequestHostObject>(this).identifiers;
    release(env, ids);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation SKProductsRequest: SKRequest
- (id)initWithProductIdentifiers:(id)ids { // NSSet *
    let ids = msg![env; ids copy];
    env.objc.borrow_mut::<RequestHostObject>(this).identifiers = ids;
    this
}
@end

};
