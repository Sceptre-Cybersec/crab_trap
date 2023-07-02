use std::{collections::HashMap};
use std::sync::Arc;

use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::socket::{connection, listener};
use futures_util::pin_mut;
use futures_util::stream::StreamExt;
use tokio::sync::{mpsc, Mutex, MutexGuard};

mod socket;

#[tokio::main]
async fn main() {
    let connected_shells = Arc::new(Mutex::new(HashMap::<String, connection::Handle>::new()));
   
    let mut menu: HashMap<
        String,
        Box<dyn Fn(MutexGuard<HashMap<String, connection::Handle>>, Option<String>) + Send + Sync + 'static>,
    > = HashMap::new();

    let list = |connected_shells: MutexGuard<HashMap<String, connection::Handle>>, _: Option<String>| {
        let shells = connected_shells.clone();
        for (key, _) in shells.into_iter() {
            println!("{}", key);
        }
    };
    menu.insert(String::from("l"), Box::new(list));

    let start = |connected_shells: MutexGuard<HashMap<String, connection::Handle>>,
                 key_opt: Option<String>| {
        if key_opt.is_some() {
            let key = key_opt.unwrap();
            let handle = match connected_shells.get(&key) {
                Some(val) => val,
                None => {
                    println!("Invalid session key!");
                    return;
                }
            };

            //start handler
            handle.egress.send("start").unwrap();
        } else {
            println!("Please provide a session key");
        }
    };

    menu.insert(String::from("s"), Box::new(start));

    let delete = |mut connected_shells: MutexGuard<HashMap<String, connection::Handle>>,
                 key_opt: Option<String>| {
        if key_opt.is_some() {
            let key = key_opt.unwrap();
            let handle = match connected_shells.get(&key) {
                Some(val) => val,
                None => {
                    println!("Invalid session key!");
                    return;
                }
            };

            //delete handler
            handle.egress.send("delete").unwrap();
            match connected_shells.remove(&key){
                Some(val)=> val.soc_kill_sig_send.send(true).unwrap(),
                None => return
            };
        } else {
            println!("Please provide a session key");
        }
    };

    menu.insert(String::from("d"), Box::new(delete));

    let help = |_: MutexGuard<HashMap<String, connection::Handle>>, _: Option<String>| {
        println!("l - list the connected shells");
        println!("h - display this help message");
        println!("s <id> - start iteration with the specified reverse shell");
        println!("d <id> - remove the specified reverse shell");
    };
    menu.insert(String::from("h"), Box::new(help.clone()));

    // start main input listener
    let connected_shells_menu = connected_shells.clone();
    tokio::spawn(async move {
        let mut stdout = io::stdout();

        loop {
            stdout.write_all("crab_trap 🦀# ".as_bytes()).await.unwrap();
            stdout.flush().await.unwrap();
            let mut reader = BufReader::new(tokio::io::stdin());
            let mut buffer = Vec::new();
            reader.read_until(b'\n', &mut buffer).await.unwrap();
            let content = match String::from_utf8(buffer) {
                Ok(val) => val,
                Err(_) => continue,
            };
            let clean_content = String::from(content.trim_end());
            let args = clean_content.split(" ").collect::<Vec<&str>>();

            let map_key = String::from(args[0]);
            let menu_arg: Option<String>;
            if args.len() > 1 {
                menu_arg = Some(String::from(args[1]));
            } else {
                menu_arg = None;
            }
            if !map_key.eq(&String::new()) {
                let menu_entry = menu.get(&map_key);
                let handle: Option<connection::Handle>;
                {
                    let shells = connected_shells_menu.lock().await;
                    handle = match shells.get(&menu_arg.clone().unwrap_or(String::new())){
                        Some(val) => Some(val.clone()),
                        None => None
                    };
                    match menu_entry {
                        Some(f) => f(shells, menu_arg),
                        None => help(shells, None),
                    };
                }
                
                if map_key.eq("s") && handle.is_some() {
                    loop {
                        let handle_clone = handle.clone();
                        match handle_clone.unwrap().ingress.lock().await.recv().await {
                            Some("pause") => break,
                            Some(_) => continue,
                            None => continue,
                        }
                    }
                }
            }
        }
    });

    let socket_stream = listener::catch_sockets(String::from("127.0.0.1"), 8080);
    pin_mut!(socket_stream);

    let connected_shells_socket = connected_shells.clone();
    loop {
        let soc = socket_stream.next().await.unwrap().unwrap();
        let (handle_to_soc_send, handle_to_soc_recv) = mpsc::channel::<String>(1024);
        let (soc_to_handle_send, mut soc_to_handle_recv) = mpsc::channel::<String>(1024);

        let (handle, handle_ingress_sender, handle_egress_receiver, soc_kill_sig_recv) = connection::Handle::new();
        {
            let mut shells = connected_shells_socket.lock().await;
            match &soc.peer_addr() {
                Ok(val) => {
                    shells.insert(val.to_string(), handle);
                }
                Err(_) => continue,
            }
        }

        
        listener::start_socket(soc, soc_to_handle_send, handle_to_soc_recv, soc_kill_sig_recv);

        let mut handle_egress_receiver_1 = handle_egress_receiver.clone();

        // start writer
        tokio::spawn(async move {
            // wait for start signal
            if listener::wait_for_signal(&mut handle_egress_receiver_1, "start")
                .await
                .is_err()
            {
                return;
            }

            loop {
                let mut reader = BufReader::new(tokio::io::stdin());
                let mut buffer = Vec::new();
                reader.read_until(b'\n', &mut buffer).await.unwrap();
                let mut content = match String::from_utf8(buffer) {
                    Ok(val) => val,
                    Err(_) => continue,
                };
                if content.trim_end().eq("quit") {

                    handle_ingress_sender.send("pause").await.unwrap();
                    // send a new line so we get a prompt when we return
                    content = String::from("\n");
                    if listener::wait_for_signal(&mut handle_egress_receiver_1, "start")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if handle_to_soc_send.send(content).await.is_err() {
                    return;
                }
            }
        });

        // start reader
        let mut handle_egress_receiver_2 = handle_egress_receiver;
        tokio::spawn(async move {
            // wait for start signal
            if listener::wait_for_signal(&mut handle_egress_receiver_2, "start")
                .await
                .is_err()
            {
                return;
            };
            println!("reader started");

            let mut stdout = io::stdout();
            loop {
                stdout.flush().await.unwrap();

                let resp = match soc_to_handle_recv.recv().await {
                    Some(val) => val,
                    None => String::from(""),
                };

                let changed = match handle_egress_receiver_2.has_changed(){
                    Ok(val) => val,
                    Err(_) => return,
                };
                if changed {
                    let val = *handle_egress_receiver_2.borrow();
                    if val.eq("pause") {
                        if listener::wait_for_signal(&mut handle_egress_receiver_2, "start")
                            .await
                            .is_err()
                        {
                            return;
                        };
                        continue;
                    }
                }
                stdout.write_all(resp.as_bytes()).await.unwrap();
            }
        });
    }
}