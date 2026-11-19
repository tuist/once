package dev.struo.mixed

class Greeter(private val name: String) {
    fun greeting(): String = Punctuation.exclaim("Hello, $name")
}
