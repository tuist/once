package dev.struo.greeting

class GreetingTest {
    fun testMessage() {
        check(Greeting.message() == "hello")
    }
}
