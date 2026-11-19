package dev.struo.mixed;

public final class Punctuation {
    private Punctuation() {}

    public static String exclaim(String text) {
        return text + "!";
    }

    public static String shout(String name) {
        return new Greeter(name).greeting().toUpperCase();
    }
}
