use crate::{
    app::{
        state::{AppState, KeyBindings},
        App, AppReturn, DateTimeFormat, VisibleBoardsAndCards,
    },
    constants::{DEFAULT_TOAST_DURATION, ENCRYPTION_KEY_FILE_NAME, FIELD_NOT_SET},
    inputs::{events::Events, key::Key, InputEvent},
    io::{
        data_handler::reset_config,
        io_handler::{
            delete_a_save_from_database, generate_new_encryption_key,
            get_all_save_ids_and_creation_dates_for_user, get_config_dir, login_for_user,
            save_user_encryption_key,
        },
        IoEvent,
    },
    ui::{
        ui_main,
        widgets::toast::{Toast, ToastType, ToastWidget},
    },
};
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use crossterm::{event::EnableMouseCapture, execute};
use eyre::Result;
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use std::{borrow::Cow, io::stdout, sync::Arc, time::Duration};
use tokio::time::Instant;

pub async fn start_ui(app: &Arc<tokio::sync::Mutex<App<'_>>>) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    {
        let app = app.lock().await;
        if app.config.enable_mouse_support {
            execute!(stdout(), EnableMouseCapture)?;
        }
    }
    let my_stdout = stdout();
    let backend = CrosstermBackend::new(my_stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    let mut events = {
        let tick_rate = app.lock().await.config.tickrate;
        Events::new(Duration::from_millis(tick_rate as u64))
    };

    {
        let mut app = app.lock().await;
        app.dispatch(IoEvent::Initialize).await;
    }

    loop {
        let mut app = app.lock().await;
        let render_start_time = Instant::now();
        terminal.draw(|rect| ui_main::draw(rect, &mut app))?;
        if app.state.ui_render_time.len() < 10 {
            app.state
                .ui_render_time
                .push(render_start_time.elapsed().as_micros());
        } else {
            app.state.ui_render_time.remove(0);
            app.state
                .ui_render_time
                .push(render_start_time.elapsed().as_micros());
        }
        // app.state.ui_render_time = Some(render_start_time.elapsed().as_micros());
        let result = match events.next().await {
            InputEvent::KeyBoardInput(key) => app.do_action(key).await,
            InputEvent::MouseAction(mouse_action) => app.handle_mouse(mouse_action).await,
            InputEvent::Tick => {
                if app.state.previous_mouse_coordinates != app.state.current_mouse_coordinates {
                    app.state.previous_mouse_coordinates = app.state.current_mouse_coordinates;
                }
                AppReturn::Continue
            }
        };
        if result == AppReturn::Exit {
            events.close();
            break;
        }
    }

    execute!(stdout(), crossterm::event::DisableMouseCapture)?;
    terminal.clear()?;
    terminal.set_cursor_position((0, 0))?;
    terminal.show_cursor()?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}

/// Takes wrapped text and the current cursor position (1D) and the available space to return the x and y position of the cursor (2D)
/// Will be replaced by a better algorithm/implementation in the future
pub fn calculate_cursor_position(
    text: Vec<Cow<str>>,
    current_cursor_position: usize,
    view_box: Rect,
) -> (u16, u16) {
    let wrapped_text_iter = text.iter();
    let mut cursor_pos = current_cursor_position;

    let mut x_pos = view_box.x + 1 + cursor_pos as u16;
    let mut y_pos = view_box.y + 1;
    for (i, line) in wrapped_text_iter.enumerate() {
        x_pos = view_box.x + 1 + cursor_pos as u16;
        y_pos = view_box.y + 1 + i as u16;
        if cursor_pos <= line.chars().count() {
            let x_pos = if x_pos > i as u16 {
                x_pos - i as u16
            } else {
                x_pos
            };
            return (x_pos, y_pos);
        }
        cursor_pos -= line.chars().count();
    }
    (x_pos, y_pos)
}

/// function to lerp between rgb values of two colors
pub fn lerp_between(
    color_a: (u8, u8, u8),
    color_b: (u8, u8, u8),
    normalized_time: f32,
) -> (u8, u8, u8) {
    // clamp the normalized time between 0 and 1
    let normalized_time = normalized_time.clamp(0.0, 1.0);
    let r = (color_a.0 as f32 * (1.0 - normalized_time) + color_b.0 as f32 * normalized_time) as u8;
    let g = (color_a.1 as f32 * (1.0 - normalized_time) + color_b.1 as f32 * normalized_time) as u8;
    let b = (color_a.2 as f32 * (1.0 - normalized_time) + color_b.2 as f32 * normalized_time) as u8;
    (r, g, b)
}

// parses a string to rgb values from a hex string
pub fn parse_hex_to_rgb(hex_string: &str) -> Option<(u8, u8, u8)> {
    if hex_string.len() != 7 {
        return None;
    }
    if !hex_string.starts_with('#') {
        return None;
    }
    let hex_string = hex_string.trim_start_matches('#');
    let r = u8::from_str_radix(&hex_string[0..2], 16);
    let g = u8::from_str_radix(&hex_string[2..4], 16);
    let b = u8::from_str_radix(&hex_string[4..6], 16);
    match (r, g, b) {
        (Ok(r), Ok(g), Ok(b)) => Some((r, g, b)),
        _ => None,
    }
}

// TODO: Find a way to get the terminal background color
pub fn get_term_bg_color() -> (u8, u8, u8) {
    (0, 0, 0)
}

pub fn date_format_finder(date_string: &str) -> Result<DateTimeFormat, String> {
    let all_formats_with_time = DateTimeFormat::all_formats_with_time();
    for date_format in DateTimeFormat::get_all_date_formats() {
        if all_formats_with_time.contains(&date_format) {
            match NaiveDateTime::parse_from_str(date_string, date_format.to_parser_string()) {
                Ok(_) => return Ok(date_format),
                Err(_) => {
                    continue;
                }
            }
        } else {
            match NaiveDate::parse_from_str(date_string, date_format.to_parser_string()) {
                Ok(_) => return Ok(date_format),
                Err(_) => {
                    continue;
                }
            }
        }
    }
    Err("Invalid date format".to_string())
}

pub fn date_format_converter(
    date_string: &str,
    date_format: DateTimeFormat,
) -> Result<String, String> {
    if date_string == FIELD_NOT_SET || date_string.is_empty() {
        return Ok(date_string.to_string());
    }
    let given_date_format = date_format_finder(date_string)?;
    if given_date_format == date_format {
        return Ok(date_string.to_string());
    }
    let all_formats_with_time = DateTimeFormat::all_formats_with_time();
    let all_formats_without_time = DateTimeFormat::all_formats_without_time();
    if all_formats_with_time.contains(&given_date_format)
        && all_formats_without_time.contains(&date_format)
    {
        let naive_date_time =
            NaiveDateTime::parse_from_str(date_string, given_date_format.to_parser_string());
        if let Ok(naive_date_time) = naive_date_time {
            let naive_date = NaiveDate::from_ymd_opt(
                naive_date_time.year(),
                naive_date_time.month(),
                naive_date_time.day(),
            );
            if let Some(naive_date) = naive_date {
                Ok(naive_date
                    .format(date_format.to_parser_string())
                    .to_string())
            } else {
                Err("Invalid date format".to_string())
            }
        } else {
            Err("Invalid date format".to_string())
        }
    } else if all_formats_without_time.contains(&given_date_format)
        && all_formats_with_time.contains(&date_format)
    {
        let naive_date =
            NaiveDate::parse_from_str(date_string, given_date_format.to_parser_string());
        if let Ok(naive_date) = naive_date {
            let default_time = NaiveTime::from_hms_opt(0, 0, 0);
            if let Some(default_time) = default_time {
                let naive_date_time = NaiveDateTime::new(naive_date, default_time);
                return Ok(naive_date_time
                    .format(date_format.to_parser_string())
                    .to_string());
            } else {
                Err("Invalid date format".to_string())
            }
        } else {
            Err("Invalid date format".to_string())
        }
    } else if all_formats_with_time.contains(&given_date_format)
        && all_formats_with_time.contains(&date_format)
    {
        let naive_date_time =
            NaiveDateTime::parse_from_str(date_string, given_date_format.to_parser_string());
        if let Ok(naive_date_time) = naive_date_time {
            return Ok(naive_date_time
                .format(date_format.to_parser_string())
                .to_string());
        } else {
            Err("Invalid date format".to_string())
        }
    } else if all_formats_without_time.contains(&given_date_format)
        && all_formats_without_time.contains(&date_format)
    {
        let naive_date =
            NaiveDate::parse_from_str(date_string, given_date_format.to_parser_string());
        if let Ok(naive_date) = naive_date {
            return Ok(naive_date
                .format(date_format.to_parser_string())
                .to_string());
        } else {
            Err("Invalid date format".to_string())
        }
    } else {
        Err("Invalid date format".to_string())
    }
}

/// only to be used as a cli argument function
pub async fn gen_new_key_main(email_id: String, password: String) -> Result<()> {
    let mut previous_key_lost = false;
    let mut key_default_path = get_config_dir().unwrap();
    key_default_path.push(ENCRYPTION_KEY_FILE_NAME);
    if key_default_path.exists() {
        print_info(
            "An encryption key already exists, are you sure you want to generate a new one? (y/n)",
        );
        println!("> ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();
        if input == "y" || input == "yes" {
            print_info("Preparing to generate new encryption key...");
        } else {
            print_info("Aborting...");
            return Ok(());
        }
    } else {
        print_warn(
            "Previous encryption key not found, preparing to generate new encryption key...",
        );
        previous_key_lost = true;
    }
    print_info("Trying to login...");
    let (access_token, user_id, _refresh_token) =
        match login_for_user(&email_id, &password, false).await {
            Ok((access_token, user_id, refresh_token)) => (access_token, user_id, refresh_token),
            Err(err) => {
                print_debug(&format!("Error logging in: {:?}", err));
                print_error("Error logging in, please check your credentials and try again");
                print_error("Aborting...");
                return Ok(());
            }
        };
    let save_ids =
        get_all_save_ids_and_creation_dates_for_user(user_id.to_owned(), &access_token, true)
            .await?;
    if save_ids.is_empty() {
        print_warn("No Cloud save files found");
        print_info("Generating new encryption key...");
        let key = generate_new_encryption_key();
        match save_user_encryption_key(&key) {
            Ok(save_location) => {
                print_info("Encryption key generated and saved");
                print_info(
                    "Please keep this key safe as it will be required to access your save files",
                );
                print_info(&format!("New Key generated_at: {}", save_location));
            }
            Err(err) => {
                print_error("Error saving encryption key");
                print_debug(&format!("Error: {:?}", err));
                return Ok(());
            }
        }
    } else {
        print_info(&format!("{} save files found", save_ids.len()));
        if previous_key_lost {
            print_warn(
                "It seems like the previous encryption key was lost as it could not be found",
            );
        }
        print_info("Cloud save files found:");
        print_info("-------------------------");
        for (i, save) in save_ids.iter().enumerate() {
            print_info(&format!(
                "{}) Cloud_save_{} - Created at (UTC) {}",
                i + 1,
                save.0,
                save.1
            ));
        }
        print_info("-------------------------");
        print_warn("Input 'Y' to delete all the save files and generate a new encryption key");
        print_info("or");
        print_info(
            format!(
                "Input 'N' to find the encryption key yourself and move it to {}",
                key_default_path.display()
            )
            .as_str(),
        );
        println!("> ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        println!();
        let input = input.trim().to_lowercase();
        if input == "y" || input == "yes" {
            for save_id in save_ids {
                print_info(&format!("Deleting save file: {}", save_id.0));
                let delete_status =
                    delete_a_save_from_database(&access_token, true, save_id.2 as u64, None).await;
                if delete_status.is_err() {
                    print_error("Error deleting save file");
                    print_debug(&format!("Error: {:?}", delete_status.err()));
                    print_error("Aborting...");
                    return Ok(());
                }
            }
            print_info("All save files deleted");
            print_info("Preparing to generate new encryption key...");
            let key = generate_new_encryption_key();
            match save_user_encryption_key(&key) {
                Ok(save_location) => {
                    print_info("Encryption key generated and saved");
                    print_info(
                        "Please keep this key safe as it will be required to access your save files",
                    );
                    print_info(&format!("New Key generated_at: {}", save_location));
                }
                Err(err) => {
                    print_error("Error saving encryption key");
                    print_debug(&format!("Error: {:?}", err));
                    return Ok(());
                }
            }
        } else {
            print_info("Aborting...");
            return Ok(());
        }
    }
    Ok(())
}

pub fn reset_app_main() {
    print_info("🚀 Resetting config");
    reset_config();
    print_info("👍 Config reset");
}

pub fn print_error(error: &str) {
    bunt::println!("{$red}[ERROR]{/$} - {}", error);
}

pub fn print_info(info: &str) {
    bunt::println!("{$cyan}[INFO]{/$}  - {}", info);
}

pub fn print_debug(debug: &str) {
    if cfg!(debug_assertions) {
        bunt::println!("{$green}[DEBUG]{/$} - {}", debug);
    }
}

pub fn print_warn(warn: &str) {
    bunt::println!("{$yellow}[WARN]{/$}  - {}", warn);
}

pub fn spaces(size: u8) -> &'static str {
    const SPACES: &str = "                                                                                                                                                                                                                                                                ";
    &SPACES[..size as usize]
}

pub fn num_digits(i: usize) -> u8 {
    f64::log10(i as f64) as u8 + 1
}

pub fn replace_tabs(s: &str, tab_len: u8) -> Cow<'_, str> {
    let tab = spaces(tab_len);
    let mut buf = String::new();
    for (i, c) in s.char_indices() {
        if buf.is_empty() {
            if c == '\t' {
                buf.reserve(s.len());
                buf.push_str(&s[..i]);
                buf.push_str(tab);
            }
        } else if c == '\t' {
            buf.push_str(tab);
        } else {
            buf.push(c);
        }
    }
    if buf.is_empty() {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(buf)
    }
}

pub fn send_info_toast(toast_widget: &mut ToastWidget, message: &str) {
    toast_widget.toasts.push(Toast::new(
        message.to_string(),
        Duration::from_secs(DEFAULT_TOAST_DURATION),
        ToastType::Info,
    ));
}

pub fn send_info_toast_with_duration(
    toast_widget: &mut ToastWidget,
    message: &str,
    duration: Duration,
) {
    toast_widget
        .toasts
        .push(Toast::new(message.to_string(), duration, ToastType::Info));
}

pub fn send_error_toast(toast_widget: &mut ToastWidget, message: &str) {
    toast_widget.toasts.push(Toast::new(
        message.to_string(),
        Duration::from_secs(DEFAULT_TOAST_DURATION),
        ToastType::Error,
    ));
}

pub fn send_error_toast_with_duration(
    toast_widget: &mut ToastWidget,
    message: &str,
    duration: Duration,
) {
    toast_widget
        .toasts
        .push(Toast::new(message.to_string(), duration, ToastType::Error));
}

pub fn send_warning_toast(toast_widget: &mut ToastWidget, message: &str) {
    toast_widget.toasts.push(Toast::new(
        message.to_string(),
        Duration::from_secs(DEFAULT_TOAST_DURATION),
        ToastType::Warning,
    ));
}

pub fn send_warning_toast_with_duration(
    toast_widget: &mut ToastWidget,
    message: &str,
    duration: Duration,
) {
    toast_widget.toasts.push(Toast::new(
        message.to_string(),
        duration,
        ToastType::Warning,
    ));
}

pub fn get_first_next_focus_keybinding(keybindings: &KeyBindings) -> &Key {
    keybindings.next_focus.first().unwrap_or(&Key::Tab)
}

pub fn get_first_prv_focus_keybinding(keybindings: &KeyBindings) -> &Key {
    keybindings.prv_focus.first().unwrap_or(&Key::BackTab)
}

/// Updates the current board and card if the board_id and card_id
pub fn update_current_board_and_card(
    app_state: &mut AppState,
    board_id: Option<(u64, u64)>,
    card_id: Option<(u64, u64)>,
) {
    app_state.current_board_id = board_id;
    app_state.current_card_id = card_id;
}

/// Updates the current visible boards and cards
pub fn update_current_visible_boards_and_cards(
    app: &mut App,
    new_visible_boards_and_cards: VisibleBoardsAndCards,
) {
    app.visible_boards_and_cards = new_visible_boards_and_cards;
}
