use crate::{
    history::{History, HistoryEntry},
    types::{MangaInfo, ShowInfo, Translation},
};

use anyhow::Result;
use dialoguer::{Confirm, FuzzySelect, Select, theme::ColorfulTheme};

pub fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

pub fn select_history_entry(history: &History) -> Result<Option<HistoryEntry>> {
    if history.entries.is_empty() {
        println!("History is empty.");
        return Ok(None);
    }

    let items: Vec<String> = history
        .entries
        .iter()
        .map(|entry| {
            let tag = if entry.is_manga {
                if entry.translation == Translation::Raw {
                    "Raw"
                } else {
                    "Man"
                }
            } else {
                entry.translation.label()
            };
            format!(
                "[{}] {} · {} {} · watched {}",
                tag,
                entry.show_title,
                if entry.is_manga { "chapter" } else { "episode" },
                entry.episode,
                entry.watched_at.format("%Y-%m-%d %H:%M")
            )
        })
        .collect();

    let selection = Select::with_theme(&theme())
        .with_prompt("Select an entry to replay (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    Ok(selection.map(|idx| history.entries[idx].clone()))
}

pub fn select_show_entry(
    shows: &Vec<ShowInfo>,
    translation: Translation,
) -> Result<Option<&ShowInfo>> {
    if shows.is_empty() {
        println!("No shows found.");
        return Ok(None);
    }

    let items: Vec<String> = shows
        .iter()
        .map(|s| {
            let count = match translation {
                Translation::Sub => s.available_eps.sub,
                Translation::Dub => s.available_eps.dub,
                Translation::Raw => 0,
            };
            format!("{} [{} episodes]", s.title, count)
        })
        .collect();

    let selection = Select::with_theme(&theme())
        .with_prompt("Select a show (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    Ok(selection.map(|idx| &shows[idx]))
}

pub fn select_manga_entry(
    mangas: &Vec<MangaInfo>,
    translation: Translation,
) -> Result<Option<&MangaInfo>> {
    if mangas.is_empty() {
        println!("No mangas found.");
        return Ok(None);
    }

    let items: Vec<String> = mangas
        .iter()
        .map(|m| {
            let count = match translation {
                Translation::Sub => m.available_chapters.sub,
                Translation::Raw => m.available_chapters.raw,
                Translation::Dub => 0,
            };
            format!("{} [{} chapters]", m.title, count)
        })
        .collect();

    let selection = Select::with_theme(&theme())
        .with_prompt("Select a manga (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    Ok(selection.map(|idx| &mangas[idx]))
}

pub fn select_episode(
    labels: &[String],
    default_idx: usize,
    prompt: &str,
) -> Result<Option<usize>> {
    if labels.is_empty() {
        return Ok(None);
    }

    Ok(FuzzySelect::with_theme(&theme())
        .with_prompt(prompt)
        .items(labels)
        .default(default_idx)
        .interact_opt()?)
}

pub fn rate(title: &str) -> Result<Option<u8>> {
    let mut rating_options: Vec<String> = (1u8..=10).map(|n| format!("{}/10", n)).collect();
    rating_options.push("Skip (no rating)".to_string());

    let rating_idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Rate \"{}\" on MAL (Esc to skip)", title))
        .items(&rating_options)
        .default(rating_options.len() - 1)
        .interact_opt()?;
    Ok(rating_idx.map(|idx| rating_options[idx].parse::<u8>().unwrap_or(0)))
}

pub fn confirm(prompt: &str) -> Result<bool> {
    Ok(Confirm::with_theme(&theme())
        .with_prompt(prompt)
        .default(false)
        .interact()?)
}
