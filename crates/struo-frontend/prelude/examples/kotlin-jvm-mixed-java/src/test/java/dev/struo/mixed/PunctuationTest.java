package dev.struo.mixed;

public class PunctuationTest {
    public void testShout() {
        String shouted = Punctuation.shout(Fixtures.NAME);
        if (!shouted.equals("HELLO, STRUO!")) {
            throw new AssertionError("unexpected greeting: " + shouted);
        }
    }
}
