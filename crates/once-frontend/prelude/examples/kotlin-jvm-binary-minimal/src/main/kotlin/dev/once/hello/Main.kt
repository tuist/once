package dev.once.hello

import dev.once.greeting.greeting

fun main(args: Array<String>) {
    println(greeting(args.firstOrNull() ?: "world"))
}
