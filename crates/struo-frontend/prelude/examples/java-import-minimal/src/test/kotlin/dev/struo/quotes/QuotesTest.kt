package dev.struo.quotes

class QuotesTest {
    fun testQuote() {
        check(Quotes.quote("Struo") == "\"Struo\"")
    }
}
