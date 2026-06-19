package dev.once.hello

import android.app.Activity
import android.os.Bundle
import android.widget.TextView
import dev.once.greeting.Greeting

class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val label = TextView(this)
        label.text = Greeting.message(getString(R.string.app_name))
        setContentView(label)
    }
}
