package dev.once.greeting

class GreetingTest {
    fun testMessage() {
        check(Greeting.message() == "hello")
    }
}
