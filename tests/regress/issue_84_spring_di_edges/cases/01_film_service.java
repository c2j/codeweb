package com.example.service;

import java.util.List;
import java.util.ArrayList;

public class FilmService {

    public List<String> getAllFilms() {
        List<String> films = new ArrayList<>();
        films.add("Inception");
        films.add("Interstellar");
        return films;
    }

    public String getFilmById(Long id) {
        return "Film[" + id + "]";
    }
}
