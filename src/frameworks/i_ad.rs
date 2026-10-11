/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Offline iAd banner stub. The iAd advertising service is no longer available.

use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::objc::{id, impl_HostObject_with_superclass, objc_classes, ClassExports, NSZonePtr};

#[derive(Default)]
struct ADBannerViewHostObject {
    superclass: UIViewHostObject,
    /// Non-retaining reference to the banner's delegate.
    delegate: id,
}
impl_HostObject_with_superclass!(ADBannerViewHostObject);

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation ADBannerView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<ADBannerViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<ADBannerViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    log!("TODO: [{:?} setDelegate:{:?}]: iAd is unavailable", this, delegate);
    env.objc.borrow_mut::<ADBannerViewHostObject>(this).delegate = delegate;
}

- (bool)isBannerLoaded {
    false
}
- (bool)isBannerViewActionInProgress {
    false
}

@end

};

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/iAd.framework/iAd",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};
