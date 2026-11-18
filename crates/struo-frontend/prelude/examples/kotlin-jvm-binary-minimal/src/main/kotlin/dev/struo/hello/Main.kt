package dev.struo.hello

import dev.struo.greeting.greeting

fun main(args: Array<String>) {
    println(greeting(args.firstOrNull() ?: "world"))
}
