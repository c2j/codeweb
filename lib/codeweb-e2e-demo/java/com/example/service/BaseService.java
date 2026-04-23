package com.example.service;

public class BaseService {
    protected void exportPdf(String content) {
        System.out.println("exporting: " + content);
    }

    protected void log(String msg) {
        System.out.println(msg);
    }
}
