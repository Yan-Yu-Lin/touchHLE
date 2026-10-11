/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Payment descriptions and offline transactions.

use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

struct PaymentHostObject {
    identifier: id,
    request_data: id,
    quantity: NSInteger,
}
impl HostObject for PaymentHostObject {}

pub(super) const SKPaymentTransactionStatePurchasing: NSInteger = 0;
pub(super) const SKPaymentTransactionStateFailed: NSInteger = 2;

pub(super) struct TransactionHostObject {
    pub payment: id,
    pub error: id,
    pub state: NSInteger,
}
impl HostObject for TransactionHostObject {}

fn copy_payment(env: &mut Environment, this: id, mutable: bool) -> id {
    let host = env.objc.borrow::<PaymentHostObject>(this);
    let (identifier, data, quantity) = (host.identifier, host.request_data, host.quantity);
    let identifier = msg![env; identifier copy];
    let request_data = msg![env; data copy];
    let new: id = if mutable {
        msg_class![env; SKMutablePayment alloc]
    } else {
        msg_class![env; SKPayment alloc]
    };
    *env.objc.borrow_mut::<PaymentHostObject>(new) = PaymentHostObject {
        identifier,
        request_data,
        quantity,
    };
    new
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation SKPayment: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(PaymentHostObject { identifier: nil, request_data: nil, quantity: 1 }), &mut env.mem)
}
+ (id)paymentWithProductIdentifier:(id)identifier {
    let new: id = msg![env; this new];
    let identifier = msg![env; identifier copy];
    env.objc.borrow_mut::<PaymentHostObject>(new).identifier = identifier;
    autorelease(env, new)
}
+ (id)paymentWithProduct:(id)product {
    let identifier: id = msg![env; product productIdentifier];
    msg![env; this paymentWithProductIdentifier:identifier]
}
- (id)productIdentifier { env.objc.borrow::<PaymentHostObject>(this).identifier }
- (id)requestData { env.objc.borrow::<PaymentHostObject>(this).request_data }
- (NSInteger)quantity { env.objc.borrow::<PaymentHostObject>(this).quantity }
- (id)copyWithZone:(NSZonePtr)_zone { copy_payment(env, this, false) }
- (id)mutableCopyWithZone:(NSZonePtr)_zone { copy_payment(env, this, true) }
- (())dealloc {
    let host = env.objc.borrow::<PaymentHostObject>(this);
    let (identifier, data) = (host.identifier, host.request_data);
    release(env, identifier);
    release(env, data);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation SKMutablePayment: SKPayment
- (())setProductIdentifier:(id)identifier {
    let identifier = msg![env; identifier copy];
    let old = std::mem::replace(&mut env.objc.borrow_mut::<PaymentHostObject>(this).identifier, identifier);
    release(env, old);
}
- (())setRequestData:(id)data {
    let data = msg![env; data copy];
    let old = std::mem::replace(&mut env.objc.borrow_mut::<PaymentHostObject>(this).request_data, data);
    release(env, old);
}
- (())setQuantity:(NSInteger)quantity {
    assert!(quantity > 0);
    env.objc.borrow_mut::<PaymentHostObject>(this).quantity = quantity;
}
@end

@implementation SKPaymentTransaction: NSObject
- (NSInteger)transactionState { env.objc.borrow::<TransactionHostObject>(this).state }
- (id)payment { env.objc.borrow::<TransactionHostObject>(this).payment }
- (id)error { env.objc.borrow::<TransactionHostObject>(this).error }
// Failed purchases have no receipt, identifier, date, or original transaction.
- (id)transactionReceipt { nil }
- (id)transactionIdentifier { nil }
- (id)transactionDate { nil }
- (id)originalTransaction { nil }
- (())dealloc {
    let host = env.objc.borrow::<TransactionHostObject>(this);
    let (payment, error) = (host.payment, host.error);
    release(env, payment);
    release(env, error);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
};

pub(super) fn new_transaction(env: &mut Environment, payment: id) -> id {
    let payment = msg![env; payment copy];
    let class = env
        .objc
        .get_known_class("SKPaymentTransaction", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(TransactionHostObject {
            payment,
            error: nil,
            state: SKPaymentTransactionStatePurchasing,
        }),
        &mut env.mem,
    )
}

pub(super) fn fail_transaction(env: &mut Environment, transaction: id) {
    let error = super::unavailable_error(env);
    retain(env, error);
    let host = env.objc.borrow_mut::<TransactionHostObject>(transaction);
    host.error = error;
    host.state = SKPaymentTransactionStateFailed;
}
