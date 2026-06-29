package dev.once.greeting

class GreetingInstrumentedTest {
    fun useAppContext() {
        check(Greeting.message() == "hello")
    }
}
