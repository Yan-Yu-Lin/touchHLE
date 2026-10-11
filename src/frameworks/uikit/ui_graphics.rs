/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGraphics.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::{
    CGColorSpaceCreateDeviceRGB, CGColorSpaceRelease,
};
use crate::frameworks::core_graphics::cg_context::{
    CGContextRef, CGContextRelease, CGContextRetain, CGContextScaleCTM, CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{kCGImageAlphaPremultipliedLast, CGImageRelease};
use crate::frameworks::core_graphics::CGSize;
use crate::mem::Ptr;
use crate::objc::{id, msg_class, nil};
use crate::Environment;

#[derive(Default)]
pub(super) struct State {
    pub(super) context_stack: Vec<CGContextRef>,
    image_contexts: Vec<CGContextRef>,
}

pub fn UIGraphicsPushContext(env: &mut Environment, context: CGContextRef) {
    CGContextRetain(env, context);
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .push(context);
}
pub fn UIGraphicsPopContext(env: &mut Environment) {
    let context = env.framework_state.uikit.ui_graphics.context_stack.pop();
    CGContextRelease(env, context.unwrap());
}
pub fn UIGraphicsGetCurrentContext(env: &mut Environment) -> CGContextRef {
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .last()
        .copied()
        .unwrap_or(nil)
}

fn UIGraphicsBeginImageContext(env: &mut Environment, size: CGSize) {
    let width = size.width.ceil() as u32;
    let height = size.height.ceil() as u32;
    let context = if width == 0 || height == 0 {
        nil
    } else {
        let color_space = CGColorSpaceCreateDeviceRGB(env);
        let context = CGBitmapContextCreate(
            env,
            Ptr::null(),
            width,
            height,
            8,
            0,
            color_space,
            kCGImageAlphaPremultipliedLast,
        );
        CGColorSpaceRelease(env, color_space);
        // UIKit's origin is the top-left corner, with y increasing downwards.
        CGContextTranslateCTM(env, context, 0.0, height as f32);
        CGContextScaleCTM(env, context, 1.0, -1.0);
        context
    };
    env.framework_state
        .uikit
        .ui_graphics
        .image_contexts
        .push(context);
    UIGraphicsPushContext(env, context);
}

fn UIGraphicsGetImageFromCurrentImageContext(env: &mut Environment) -> id {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil
        || env.framework_state.uikit.ui_graphics.image_contexts.last() != Some(&context)
    {
        return nil;
    }
    let image = CGBitmapContextCreateImage(env, context);
    let result = msg_class![env; UIImage imageWithCGImage:image];
    CGImageRelease(env, image);
    result
}

fn UIGraphicsEndImageContext(env: &mut Environment) {
    let Some(context) = env.framework_state.uikit.ui_graphics.image_contexts.pop() else {
        return;
    };
    assert_eq!(UIGraphicsGetCurrentContext(env), context);
    UIGraphicsPopContext(env);
    CGContextRelease(env, context);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIGraphicsPushContext(_)),
    export_c_func!(UIGraphicsPopContext()),
    export_c_func!(UIGraphicsGetCurrentContext()),
    export_c_func!(UIGraphicsBeginImageContext(_)),
    export_c_func!(UIGraphicsGetImageFromCurrentImageContext()),
    export_c_func!(UIGraphicsEndImageContext()),
];
