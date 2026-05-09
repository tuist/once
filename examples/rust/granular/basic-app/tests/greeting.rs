extern crate greeting;

#[test]
fn builds_a_message() {
    assert_eq!(greeting::message("agent"), "Hello, agent, from Fabrik");
}
