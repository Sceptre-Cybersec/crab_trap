use std::cmp::max;
use std::collections::HashMap;

use std::io::{stdin, stdout, Stdout, Write};
use std::sync::Arc;
use termion::cursor::DetectCursorPos;
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::{IntoRawMode, RawTerminal};
use termion::{clear, color, cursor, terminal_size};
use tokio::sync::{mpsc, Mutex, MutexGuard};

use crate::socket::connection;

pub type MenuListValue = Box<
    dyn Fn(Arc<Mutex<HashMap<String, connection::Handle>>>, mpsc::Sender<()>)
        + Send
        + Sync
        + 'static,
>;

pub type MenuList = HashMap<&'static str, MenuListValue>;

pub fn help() {
    println!("l - list the connected shells");
    println!("h - display this help message");
    println!("clear - clear the display");
}

pub fn clear() {
    println!("{clear}", clear = clear::All);
}

fn start(key: String, connected_shells: &MutexGuard<HashMap<String, connection::Handle>>) {
    let handle = match connected_shells.get(&key) {
        Some(val) => val,
        None => {
            println!("Invalid session key!");
            return;
        }
    };

    //start handler
    let mut stdout = stdout();
    write!(stdout, "{clear}", clear = clear::BeforeCursor).unwrap();
    if handle.raw_mode {
        write!(
            stdout,
            "\r\n{guide}type \"CTRL + b\" to return to menu{reset}\r\n",
            guide = color::Fg(color::Red),
            reset = color::Fg(color::Reset)
        )
        .unwrap();
    } else {
        write!(
            stdout,
            "\r\n{guide}type \"back\" to return to menu{reset}\r\n",
            guide = color::Fg(color::Red),
            reset = color::Fg(color::Reset)
        )
        .unwrap();
    }
    handle.tx.send("start").unwrap();
}

fn delete(key: String, connected_shells: &mut MutexGuard<HashMap<String, connection::Handle>>) {
    let handle = match connected_shells.remove(&key) {
        Some(val) => val,
        None => {
            println!("Invalid session key!");
            return;
        }
    };
    //delete handler
    handle.tx.send("delete").unwrap();
    handle.soc_kill_token.cancel();
}

fn alias(
    shell_key: String,
    selected_index: u16,
    connected_shells: &mut MutexGuard<HashMap<String, connection::Handle>>,
) {
    let mut stdout = stdout();
    macro_rules! reset_alias_line {
        ($stdout:expr, $prompt:expr $(, $input:expr)?) => {
            write!(
                $stdout,
                "{goto}{prompt}{blink}{clear}",
                prompt = $prompt,
                goto = cursor::Goto(0, selected_index),
                clear = clear::AfterCursor,
                blink = cursor::BlinkingBlock
            )
            .unwrap();
            #[allow(unused_mut, unused_assignments)]
            let mut input = String::new();
            $ ( input = $input; )?
            if !input.is_empty(){
                write!(
                    $stdout,
                    "{input}{blink}{clear}",
                    input = input,
                    blink = cursor::BlinkingBlock,
                    clear = clear::AfterCursor
                ).unwrap();
            }
            $stdout.flush().unwrap();
        };
    }
    let mut prompt = String::from("Please enter a new alias: ");
    reset_alias_line!(stdout, prompt);
    let mut input = String::new();
    for key in stdin().keys() {
        match key.unwrap() {
            Key::Char(c) => {
                if c == '\n' || c == '\r' {
                    if input.is_empty() {
                        prompt = String::from("❌ Alias cannot be empty, please try again: ")
                    } else if connected_shells.contains_key(&input) {
                        input = String::new();
                        prompt = String::from("❌ Alias already exists, please try again: ")
                    } else {
                        let shell = connected_shells.remove(&shell_key).unwrap();
                        connected_shells.insert(input, shell);
                        return;
                    }
                } else {
                    input += &c.to_string();
                }
            }
            Key::Backspace | Key::Delete => {
                if input.len() > 0 {
                    input = String::from(&input[..input.len() - 1]);
                }
            }
            Key::Esc => {
                return;
            }
            _ => {}
        }
        reset_alias_line!(stdout, prompt, input.clone());
        prompt = String::from("Please enter a new alias: ")
    }
}

macro_rules! unlock_menu {
    ($menu_channel_release:expr) => {
        println!(
            "\r\n{show}{blink}{clear}",
            show = cursor::Show,
            blink = cursor::BlinkingBlock,
            clear = clear::AfterCursor
        );
        let menu_esc_release = $menu_channel_release.clone();
        tokio::spawn(async move {
            menu_esc_release.send(()).await.unwrap();
            return;
        });
    };
}

fn list_menu_help(stdout: &mut RawTerminal<Stdout>) {
    let (width, start_pos) = terminal_size().unwrap();
    let msgs = [
        String::from("(ENTER - start shell) (DEL | BACK - remove shell) (ESC - back to menu)"),
        String::from("(a - rename shell) (r - enter tty (raw) mode)"),
    ];
    for msg in msgs{
        let mut display_msg = msg.clone();
        if msg.len() > width.into() {
            let split = max(width - 3, 0).into();
            display_msg = String::from(&msg.as_str()[..split]);
            display_msg = display_msg + "...";
        }
        write!(
            stdout,
            "\r\n{goto}{select}{msg}{reset}",
            goto = cursor::Goto(0, start_pos),
            msg = display_msg,
            select = color::Bg(color::LightBlack),
            reset = color::Bg(color::Reset)
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}

fn refresh_list_display(
    stdout: &mut RawTerminal<Stdout>,
    cur_idx: usize,
    keys: Vec<(String, connection::Handle)>,
) {
    write!(
        stdout,
        "{goto}{clear}{clear_before}",
        goto = cursor::Goto(0, 2 as u16),
        clear = clear::AfterCursor,
        clear_before = clear::BeforeCursor
    )
    .unwrap();
    stdout.flush().unwrap();
    for (i, key) in keys.clone().into_iter().enumerate() {
        let raw_mode = match key.1.raw_mode {
            true => " (raw)",
            false => "",
        };
        let selection: String;
        if i == cur_idx {
            selection = format!(
                "{select}{key}{raw}{reset}{hide}",
                key = key.0,
                raw = raw_mode,
                select = color::Bg(color::Red),
                hide = cursor::Hide,
                reset = color::Bg(color::Reset),
            );
        } else {
            selection = format!(
                "{key}{raw}{hide}",
                key = key.0,
                raw = raw_mode,
                hide = cursor::Hide,
            );
        }
        write!(stdout, "{}", selection).unwrap();
        if i < &keys.len() - 1 {
            write!(stdout, "\r\n{clear}", clear = clear::AfterCursor).unwrap();
        }
        stdout.flush().unwrap();
    }
    list_menu_help(stdout);

}

pub fn new() -> MenuList {
    let mut menu: MenuList = HashMap::new();

    let list = |connected_shells: Arc<Mutex<HashMap<String, connection::Handle>>>,
                menu_channel_release: mpsc::Sender<()>| {
        tokio::spawn(async move {
            let stdin = stdin();
            let mut stdout = stdout().into_raw_mode().unwrap();
            let mut shell_list: Vec<(String, connection::Handle)>;
            {
                shell_list = connected_shells
                    .lock()
                    .await
                    .iter()
                    .map(|item| (item.0.to_owned(), item.1.to_owned()))
                    .collect::<Vec<(String, connection::Handle)>>()
            }
            if shell_list.len() > 0 {
                let (_, start_pos) = stdout.cursor_pos().unwrap();
                let mut cur_idx = 0;
                let mut keys: Vec<String>;

                keys = shell_list.iter().map(|item| item.0.to_owned()).collect();
                refresh_list_display(&mut stdout, cur_idx, shell_list.to_owned());

                let mut line_offset: i16 = 0;
                let mut shells = connected_shells.lock().await;
                for key in stdin.keys() {
                    {
                        match key.unwrap() {
                            Key::Esc => {
                                unlock_menu!(menu_channel_release);
                                return;
                            }
                            Key::Up => {
                                if cur_idx > 0 {
                                    cur_idx -= 1;
                                }
                            }
                            Key::Down => {
                                if cur_idx < keys.len() - 1 {
                                    cur_idx += 1;
                                }
                            }
                            Key::Char('\n') | Key::Char('\r') => {
                                let key = keys[cur_idx].to_owned();
                                start(key, &shells);
                                println!(
                                    "\r\n{show}{blink}",
                                    show = cursor::Show,
                                    blink = cursor::BlinkingBlock
                                );
                                return;
                            }
                            Key::Char('r') => {
                                // activate raw mode
                                let key: String = keys[cur_idx].to_owned();
                                let handle = match shells.get_mut(&key) {
                                    Some(han) => han,
                                    None => return,
                                };
                                handle.raw_mode = !handle.raw_mode;
                                handle.tx.send("raw").unwrap();
                            }
                            Key::Delete | Key::Backspace => {
                                let key: String = keys[cur_idx].to_owned();
                                delete(key, &mut shells);
                                line_offset -= 1;
                                if cur_idx > 0 {
                                    cur_idx -= 1;
                                }
                                write!(stdout, "{}", clear::CurrentLine).unwrap();
                                stdout.flush().unwrap();

                                if shells.is_empty() {
                                    unlock_menu!(menu_channel_release);
                                    return;
                                }
                            }
                            Key::Char('a') => {
                                let key: String = keys[cur_idx].to_owned();

                                alias(
                                    key,
                                    (((start_pos + cur_idx as u16) as i16 + line_offset)
                                        - (shells.len() as i16))
                                        as u16,
                                    &mut shells,
                                );
                            }
                            _ => {}
                        }
                    }
                    shell_list = shells
                        .iter()
                        .map(|item| (item.0.to_owned(), item.1.to_owned()))
                        .collect();
                    keys = shell_list.iter().map(|item| item.0.to_owned()).collect();
                    refresh_list_display(
                        &mut stdout,
                        cur_idx,
                        shell_list,
                    );
                }
            }
            unlock_menu!(menu_channel_release);
        });
    };
    menu.insert("l", Box::new(list));

    let clear = |_, _| {
        clear();
    };

    menu.insert("clear", Box::new(clear));

    menu.insert("h", Box::new(|_, _| help()));

    return menu;
}
