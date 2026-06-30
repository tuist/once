use hello::greeting;

#[test]
fn returns_greeting() {
    assert_eq!(greeting(), "hello");
}
