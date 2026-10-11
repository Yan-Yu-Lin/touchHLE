/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Offline payment queue. Payments always fail; no purchases are granted.

use super::sk_payment::{self, SKPaymentTransactionStateFailed};
use crate::frameworks::foundation::{ns_array, NSInteger};
use crate::objc::{
    autorelease, id, msg, msg_send, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct QueueHostObject {
    observers: Vec<id>, // Non-retaining, as on StoreKit's payment queue.
    transactions: Vec<id>,
    restoring: bool,
}
impl HostObject for QueueHostObject {}

fn schedule(env: &mut Environment, queue: id, selector: &str, argument: id) {
    let selector = env.objc.lookup_selector(selector).unwrap();
    () = msg![env; queue performSelectorOnMainThread:selector withObject:argument waitUntilDone:false];
}

fn array(env: &mut Environment, objects: Vec<id>) -> id {
    for &object in &objects {
        retain(env, object);
    }
    ns_array::from_vec(env, objects)
}

fn notify(env: &mut Environment, queue: id, name: &str, argument: id) {
    let observers = env.objc.borrow::<QueueHostObject>(queue).observers.clone();
    // Callbacks can finish transactions and remove observers. Keep the entire
    // snapshot alive, and check membership again before each notification.
    for &observer in &observers {
        retain(env, observer);
    }
    let selector = env
        .objc
        .register_host_selector(name.to_owned(), &mut env.mem);
    for observer in observers {
        if env
            .objc
            .borrow::<QueueHostObject>(queue)
            .observers
            .contains(&observer)
            && msg![env; observer respondsToSelector:selector]
        {
            () = msg_send(env, (observer, selector, queue, argument));
        }
        release(env, observer);
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKPaymentQueue: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<QueueHostObject>::default(), &mut env.mem)
}

+ (id)defaultQueue {
    if let Some(queue) = env.framework_state.store_kit.default_queue {
        return queue;
    }
    let queue = msg![env; this new];
    env.framework_state.store_kit.default_queue = Some(queue);
    queue
}

+ (bool)canMakePayments {
    false
}

- (())addTransactionObserver:(id)observer {
    let observers = &mut env.objc.borrow_mut::<QueueHostObject>(this).observers;
    if observer == nil || observers.contains(&observer) { return; }
    observers.push(observer);
    schedule(env, this, "_touchHLE_notifyObserver:", observer);
}
- (())removeTransactionObserver:(id)observer {
    env.objc.borrow_mut::<QueueHostObject>(this).observers.retain(|&o| o != observer);
}
- (id)transactions {
    let transactions = env.objc.borrow::<QueueHostObject>(this).transactions.clone();
    let result = array(env, transactions);
    autorelease(env, result)
}
- (())addPayment:(id)payment {
    assert!(payment != nil);
    let transaction = sk_payment::new_transaction(env, payment);
    env.objc.borrow_mut::<QueueHostObject>(this).transactions.push(transaction);
    log!("StoreKit purchases unavailable: queued transaction {:?}", transaction);
    schedule(env, this, "_touchHLE_failPayment:", transaction);
}
- (())_touchHLE_failPayment:(id)transaction {
    if !env.objc.borrow::<QueueHostObject>(this).transactions.contains(&transaction) { return; }
    sk_payment::fail_transaction(env, transaction);
    let transactions = array(env, vec![transaction]);
    log!("StoreKit transaction {:?}: Failed (SKErrorPaymentNotAllowed)", transaction);
    notify(env, this, "paymentQueue:updatedTransactions:", transactions);
    release(env, transactions);
}
- (())_touchHLE_notifyObserver:(id)observer {
    if !env.objc.borrow::<QueueHostObject>(this).observers.contains(&observer) { return; }
    let transactions = env.objc.borrow::<QueueHostObject>(this).transactions.clone();
    let transactions: Vec<_> = transactions.into_iter().filter(|&t| {
        let state: NSInteger = msg![env; t transactionState];
        state == SKPaymentTransactionStateFailed
    }).collect();
    if transactions.is_empty() { return; }
    let transactions = array(env, transactions);
    () = msg![env; observer paymentQueue:this updatedTransactions:transactions];
    release(env, transactions);
}
- (())finishTransaction:(id)transaction {
    let transactions = &mut env.objc.borrow_mut::<QueueHostObject>(this).transactions;
    let Some(index) = transactions.iter().position(|&t| t == transaction) else { return; };
    let state: NSInteger = msg![env; transaction transactionState];
    assert_eq!(state, SKPaymentTransactionStateFailed);
    env.objc.borrow_mut::<QueueHostObject>(this).transactions.remove(index);
    schedule(env, this, "_touchHLE_removedTransaction:", transaction);
    log!("StoreKit transaction {:?}: finished and removed", transaction);
    release(env, transaction);
}
- (())_touchHLE_removedTransaction:(id)transaction {
    let transactions = array(env, vec![transaction]);
    notify(env, this, "paymentQueue:removedTransactions:", transactions);
    release(env, transactions);
}
- (())restoreCompletedTransactions {
    let host = env.objc.borrow_mut::<QueueHostObject>(this);
    if host.restoring { return; }
    host.restoring = true;
    schedule(env, this, "_touchHLE_restoreFailed:", nil);
}
- (())_touchHLE_restoreFailed:(id)_unused {
    env.objc.borrow_mut::<QueueHostObject>(this).restoring = false;
    let error = super::unavailable_error(env);
    notify(env, this, "paymentQueue:restoreCompletedTransactionsFailedWithError:", error);
}
- (())dealloc {
    let transactions = std::mem::take(&mut env.objc.borrow_mut::<QueueHostObject>(this).transactions);
    for transaction in transactions { release(env, transaction); }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
