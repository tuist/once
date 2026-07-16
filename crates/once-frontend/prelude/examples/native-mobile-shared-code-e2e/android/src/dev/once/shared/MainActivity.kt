package dev.once.shared

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

class MainActivity : Activity() {
    private external fun swiftAnswer(): Int
    private external fun rustAnswer(): Int

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        System.loadLibrary("SharedSwift")
        System.loadLibrary("shared_rust")

        val label = TextView(this)
        label.text = "Swift ${swiftAnswer()} · Rust ${rustAnswer()}"
        setContentView(label)
    }
}
