package com.example.controller;

import com.example.service.FilmService;

public class FilmControllerCdi {

    private final FilmService filmService;

    public FilmControllerCdi(FilmService filmService) {
        this.filmService = filmService;
    }

    public void listFilms() {
    }
}
