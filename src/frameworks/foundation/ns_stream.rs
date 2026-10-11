/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Synchronous file-backed `NSOutputStream` support.
//! TODO: input, memory and network streams, and run-loop events.

use super::{ns_string, NSInteger, NSUInteger};
use crate::libc::{errno, posix_io};
use crate::mem::{ConstPtr, ConstVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject,
    NSZonePtr,
};

const NSStreamStatusNotOpen: NSUInteger = 0;
const NSStreamStatusOpen: NSUInteger = 2;
const NSStreamStatusClosed: NSUInteger = 6;
const NSStreamStatusError: NSUInteger = 7;

struct OutputStreamHostObject {
    path: id,
    append: bool,
    fd: Option<posix_io::FileDescriptor>,
    status: NSUInteger,
    error: i32,
}
impl HostObject for OutputStreamHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSStream: NSObject
@end

@implementation NSOutputStream: NSStream

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = OutputStreamHostObject {
        path: nil,
        append: false,
        fd: None,
        status: NSStreamStatusNotOpen,
        error: 0,
    };
    env.objc.alloc_object(this, Box::new(host), &mut env.mem)
}

+ (id)outputStreamToFileAtPath:(id)path append:(bool)append {
    let stream: id = msg![env; this alloc];
    let stream: id = msg![env; stream initToFileAtPath:path append:append];
    autorelease(env, stream)
}

- (id)initToFileAtPath:(id)path append:(bool)append {
    let path: id = msg![env; path copy];
    let host = env.objc.borrow_mut::<OutputStreamHostObject>(this);
    host.path = path;
    host.append = append;
    this
}

- (())open {
    let host = env.objc.borrow::<OutputStreamHostObject>(this);
    if host.status != NSStreamStatusNotOpen {
        return;
    }
    let (path, append) = (host.path, host.append);
    let path: ConstPtr<u8> = msg![env; path UTF8String];
    let flags = posix_io::O_WRONLY | posix_io::O_CREAT |
        if append { posix_io::O_APPEND } else { posix_io::O_TRUNC };
    let fd = posix_io::open_direct(env, path, flags);
    let error = if fd == -1 { errno::get_errno(env) } else { 0 };
    let host = env.objc.borrow_mut::<OutputStreamHostObject>(this);
    if fd == -1 {
        host.status = NSStreamStatusError;
        host.error = if error == 0 { errno::EIO } else { error };
    } else {
        host.fd = Some(fd);
        host.status = NSStreamStatusOpen;
    }
}

- (NSInteger)write:(ConstVoidPtr)buffer maxLength:(NSUInteger)length {
    let host = env.objc.borrow::<OutputStreamHostObject>(this);
    if host.status != NSStreamStatusOpen {
        return -1;
    }
    let fd = host.fd.unwrap();
    let result = posix_io::write(env, fd, buffer, length);
    if result == -1 {
        let error = errno::get_errno(env);
        let host = env.objc.borrow_mut::<OutputStreamHostObject>(this);
        host.status = NSStreamStatusError;
        host.error = if error == 0 { errno::EIO } else { error };
    }
    result
}

- (bool)hasSpaceAvailable {
    env.objc.borrow::<OutputStreamHostObject>(this).status == NSStreamStatusOpen
}
- (NSUInteger)streamStatus {
    env.objc.borrow::<OutputStreamHostObject>(this).status
}
- (id)streamError {
    let error = env.objc.borrow::<OutputStreamHostObject>(this).error;
    if error == 0 {
        return nil;
    }
    let domain = ns_string::get_static_str(env, "NSPOSIXErrorDomain");
    msg_class![env; NSError errorWithDomain:domain code:error userInfo:nil]
}

- (())close {
    let host = env.objc.borrow_mut::<OutputStreamHostObject>(this);
    let fd = host.fd.take();
    host.status = NSStreamStatusClosed;
    if let Some(fd) = fd {
        posix_io::close(env, fd);
    }
}

- (())dealloc {
    () = msg![env; this close];
    let path = env.objc.borrow::<OutputStreamHostObject>(this).path;
    release(env, path);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
