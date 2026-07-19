package dev.once.greeting

class GreetingTest {
    fun testGreeting() {
        check(greeting("Once") == "Hello, Once!")
    }
}
