package dev.struo.greeting

class GreetingTest {
    fun testGreeting() {
        check(greeting("Struo") == "Hello, Struo!")
    }
}
