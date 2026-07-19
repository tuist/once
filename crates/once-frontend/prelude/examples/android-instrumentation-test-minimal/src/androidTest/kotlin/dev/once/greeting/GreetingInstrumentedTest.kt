package dev.once.greeting

import android.app.Activity
import android.app.Instrumentation
import android.os.Bundle

class GreetingInstrumentedTest {
    fun testGreeting() {
        check("hello" == "hello")
    }
}

class OnceInstrumentationRunner : Instrumentation() {
    private lateinit var runnerArguments: Bundle

    override fun onCreate(arguments: Bundle) {
        runnerArguments = arguments
        start()
    }

    override fun onStart() {
        val requested = runnerArguments.getString("class").orEmpty()
        val requestedClass = requested.substringBefore('#')
        val requestedMethod = requested.substringAfter('#', "")
        var failures = 0
        var executed = 0

        for (testClass in listOf(GreetingInstrumentedTest::class.java)) {
            if (requestedClass.isNotEmpty() && requestedClass != testClass.name) continue
            for (method in testClass.declaredMethods.sortedBy { it.name }) {
                if (!method.name.startsWith("test") || method.parameterCount != 0) continue
                if (requestedMethod.isNotEmpty() && requestedMethod != method.name) continue
                val status = Bundle().apply {
                    putString("class", testClass.name)
                    putString("test", method.name)
                }
                sendStatus(1, status)
                executed++
                try {
                    method.invoke(testClass.getDeclaredConstructor().newInstance())
                    sendStatus(0, status)
                } catch (error: Throwable) {
                    failures++
                    status.putString("stack", error.stackTraceToString())
                    sendStatus(-2, status)
                }
            }
        }

        val result = Bundle()
        if (executed == 0) result.putString("shortMsg", "No tests matched")
        finish(if (failures == 0 && executed > 0) Activity.RESULT_OK else Activity.RESULT_CANCELED, result)
    }
}
