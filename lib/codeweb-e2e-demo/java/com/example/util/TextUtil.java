package com.example.util;

public class TextUtil {
    public static String trim(String input) {
        return input == null ? "" : input.trim();
    }
    public static String sanitize(String input) {
        return trim(input).replaceAll("[<>]", "");
    }
}
