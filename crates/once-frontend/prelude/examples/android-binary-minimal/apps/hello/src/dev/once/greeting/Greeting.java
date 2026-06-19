package dev.once.greeting;

public final class Greeting {
    private Greeting() {
    }

    public static String message(String appName) {
        return appName + " from Java";
    }
}
