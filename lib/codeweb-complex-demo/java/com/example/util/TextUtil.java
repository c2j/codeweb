package com.example.util;

public class TextUtil {
    public String sanitize(String input) {
        return input.trim();
    }

    public String truncate(String input, int maxLen) {
        return input.substring(0, maxLen);
    }
}
