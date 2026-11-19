package dev.struo.mixed

class GreeterTest {
    fun testGreeting() {
        check(Greeter(Fixtures.NAME).greeting() == "Hello, Struo!")
    }
}
