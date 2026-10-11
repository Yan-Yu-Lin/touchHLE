/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! StoreKit

mod sk_payment;
mod sk_payment_queue;
mod sk_product;

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::objc::{id, msg_class};
use crate::Environment;

#[derive(Default)]
pub struct State {
    default_queue: Option<id>,
}

const SKErrorDomain: &str = "SKErrorDomain";
const SKErrorPaymentNotAllowed: i32 = 4;

/// Autoreleased error for the unavailable App Store service.
fn unavailable_error(env: &mut Environment) -> id {
    let domain = ns_string::get_static_str(env, SKErrorDomain);
    let key = ns_string::get_static_str(env, "NSLocalizedDescriptionKey");
    let description = ns_string::get_static_str(env, "In-app purchases are unavailable.");
    let info: id = msg_class![env; NSDictionary dictionaryWithObject:description forKey:key];
    msg_class![env; NSError errorWithDomain:domain code:SKErrorPaymentNotAllowed userInfo:info]
}

const CONSTANTS: ConstantExports = &[("_SKErrorDomain", HostConstant::NSString(SKErrorDomain))];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/StoreKit.framework/StoreKit",
    aliases: &[],
    class_exports: &[
        sk_payment::CLASSES,
        sk_payment_queue::CLASSES,
        sk_product::CLASSES,
    ],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
