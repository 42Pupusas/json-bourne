//! Nesting-stack tests: `Stack`/`Frame` bit packing and the depth limit.

use crate::ErrorKind;
use crate::lexer::{Frame, Stack};

#[test]
fn pop_after_mixed_pushes_returns_frame_kinds() {
    let mut s = Stack::<8>::new();
    s.push(Frame::Object).unwrap();
    s.push(Frame::Array).unwrap();
    s.push(Frame::Object).unwrap();
    assert_eq!(s.top(), Some(Frame::Object));
    assert_eq!(s.pop(), Some(Frame::Object));
    assert_eq!(s.pop(), Some(Frame::Array));
    assert_eq!(s.pop(), Some(Frame::Object));
    assert_eq!(s.pop(), None);
}

#[test]
fn array_push_over_a_popped_object_bit() {
    // The stale-bit case: an Object frame leaves its bit set when
    // popped; re-pushing an Array at the same depth must clear it.
    let mut s = Stack::<8>::new();
    s.push(Frame::Object).unwrap();
    s.push(Frame::Array).unwrap();
    s.pop().unwrap();
    assert_eq!(s.top(), Some(Frame::Object));
    s.push(Frame::Array).unwrap();
    assert_eq!(s.top(), Some(Frame::Array), "stale Object bit resurrected");
    assert_eq!(s.pop(), Some(Frame::Array));
}

#[test]
fn truncate_then_push_writes_fresh_frames() {
    let mut s = Stack::<8>::new();
    s.push(Frame::Object).unwrap();
    s.push(Frame::Object).unwrap();
    s.truncate(1);
    assert_eq!(s.len(), 1);
    s.push(Frame::Array).unwrap();
    assert_eq!(s.top(), Some(Frame::Array));
    assert_eq!(s.pop(), Some(Frame::Array));
    assert_eq!(s.pop(), Some(Frame::Object));
}

#[test]
fn truncate_past_len_is_a_no_op() {
    let mut s = Stack::<8>::new();
    s.push(Frame::Array).unwrap();
    s.truncate(5);
    assert_eq!(s.len(), 1);
}

#[test]
fn push_past_capacity_errors() {
    let mut s = Stack::<2>::new();
    assert!(s.push(Frame::Array).is_ok());
    assert!(s.push(Frame::Object).is_ok());
    assert!(s.push(Frame::Array).is_err());
    assert_eq!(s.len(), 2);
}

#[test]
fn parser_reports_depth_limit_over_128() {
    let depth = 200;
    let input = "[".repeat(depth);
    let mut p: json_bourne::Parser<'_, 128> = json_bourne::Parser::new(input.as_bytes());
    let mut hit_limit = false;
    while let Some(ev) = p.next_event().transpose() {
        match ev {
            Ok(_) => {}
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::DepthLimitExceeded);
                hit_limit = true;
                break;
            }
        }
    }
    assert!(hit_limit, "200-deep input must trip the 128 limit");
}
