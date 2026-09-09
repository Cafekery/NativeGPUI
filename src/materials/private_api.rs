use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Bool, MessageReceiver, Sel};
use objc2::{Message, msg_send};
use objc2_foundation::NSString;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Shape {
    TakesInteger,
    TakesObject,
    ReturnsInteger,
    ReturnsObject,
}

fn gate<T: Message>(object: &T, selector: Sel, shape: Shape) -> bool {
    thread_local! {
        static CACHE: RefCell<HashMap<(usize, Sel, Shape), bool>> = RefCell::new(HashMap::new());
    }
    let class: *const AnyClass = unsafe { msg_send![object, class] };
    let Some(class) = (unsafe { class.as_ref() }) else {
        return false;
    };
    let key = (class as *const AnyClass as usize, selector, shape);
    if let Some(answer) = CACHE.with(|cache| cache.borrow().get(&key).copied()) {
        return answer;
    }
    let integer = |encoding: &[u8]| encoding.len() == 1 && b"qQiIlLsScCB".contains(&encoding[0]);
    let object_encoding = |encoding: &[u8]| encoding.first() == Some(&b'@');
    let answer = class
        .instance_method(selector)
        .is_some_and(|method| match shape {
            Shape::TakesInteger => method
                .argument_type(2)
                .is_some_and(|encoding| integer(encoding.to_bytes())),
            Shape::TakesObject => method
                .argument_type(2)
                .is_some_and(|encoding| object_encoding(encoding.to_bytes())),
            Shape::ReturnsInteger => integer(method.return_type().to_bytes()),
            Shape::ReturnsObject => object_encoding(method.return_type().to_bytes()),
        });
    CACHE.with(|cache| cache.borrow_mut().insert(key, answer));
    answer
}

pub(crate) fn send_isize<T: Message>(object: &T, selector: Sel, value: isize) {
    if gate(object, selector, Shape::TakesInteger) {
        unsafe { object.send_message(selector, (value,)) }
    }
}

pub(crate) fn send_bool<T: Message>(object: &T, selector: Sel, value: bool) {
    if gate(object, selector, Shape::TakesInteger) {
        unsafe { object.send_message(selector, (Bool::new(value),)) }
    }
}

pub(crate) fn send_string<T: Message>(object: &T, selector: Sel, value: Option<&NSString>) {
    if gate(object, selector, Shape::TakesObject) {
        let value: *const NSString = value.map_or(std::ptr::null(), |value| value);
        unsafe { object.send_message(selector, (value,)) }
    }
}

pub(crate) fn read_isize<T: Message>(object: &T, selector: Sel) -> Option<isize> {
    gate(object, selector, Shape::ReturnsInteger)
        .then(|| unsafe { object.send_message(selector, ()) })
}

pub(crate) fn read_string<T: Message>(object: &T, selector: Sel) -> Option<Retained<NSString>> {
    if !gate(object, selector, Shape::ReturnsObject) {
        return None;
    }
    let value: *mut NSString = unsafe { object.send_message(selector, ()) };
    unsafe { Retained::retain(value) }
}
