#[macro_use] extern crate rocket;
use std::{collections::{HashMap, LinkedList}, fs::{exists, File}, hash::Hash, io::{self, Write}, sync::Arc};
use std::fs;
use futures::{channel::mpsc::{unbounded, UnboundedSender}, future::select, lock::Mutex, select};
use rocket::State;
use serde::{ Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
struct User {
    name:  String,
    password: String
}

struct UserManager {
    users: HashMap<String, User>,
    onlines: HashMap<String, UnboundedSender<String>>,
    file: String
}

enum Command {
    Register{
        name: String, 
        password: String
    },
    Login{
        name: String,
        password: String
    },
    Text {
        name: String,
        content: String
    },
    Quit
}

fn parseReg(line: &str) -> Option<Command> {
    let args : Vec<&str> = line.split_ascii_whitespace().collect();
    if args.len() > 4 && args[0] == "reg" && args[1] == "-u" && args[3] == "-p" {
        return Some(Command::Register { name: args[2].to_string(), password: args[4].to_string() });
    }
    None
}

fn parseQuit(line: &str) -> Option<Command> {
    let args : Vec<&str> = line.split_ascii_whitespace().collect();
    if args.len() > 0 && args[0] == "quit" {
        return Some(Command::Quit);
    }
    None
}

fn parseLogin(line: &str) -> Option<Command> {
    let args : Vec<&str> = line.split_ascii_whitespace().collect();
    if args.len() > 4 && args[0] == "login" && args[1] == "-u" && args[3] == "-p" {
        return Some(Command::Login { name: args[2].to_string(), password: args[4].to_string() });
    }
    None
}

fn parseText(line: &str) -> Option<Command> {
    let args : Vec<&str> = line.split_ascii_whitespace().collect();
    if args.len() > 3 && args[0] == "text" && args[1] == "-u" {
        return Some(Command::Text { name: args[2].to_string(), content: args[3].to_string() });
    }
    None
}

fn parseCommand(cmd: &str) -> Option<Command> {
    parseReg(cmd).or(parseLogin(cmd)).or(parseText(cmd)).or(parseQuit(cmd))
}

type ChatError = Box<dyn std::error::Error + 'static>;

impl UserManager {
    fn new(file_path: &str) -> UserManager {
        UserManager {
            users: HashMap::new(),
            file: file_path.to_string(),
            onlines: HashMap::new()
        }
    }

    fn load(file_path: &str) -> Result<UserManager, ChatError> {
        if let Ok(true) =  fs::exists(file_path){
            let content = fs::read_to_string(file_path)?;
            let users: Vec<User> = serde_json::from_str(&content).map_err(|e| io::Error::from(e))?;
            let mut user_manager = UserManager::new(file_path);
            user_manager.users = HashMap::from_iter(users.into_iter().map(|x | (x.name.clone(), x)));
            Ok(user_manager)
        } else {
            return Ok(UserManager::new(file_path));
        }
    }

    fn save(self: &Self) -> Result<(), ChatError> {
        let mut f = File::create(self.file.as_str())?;
        let um = self.users.clone();
        let us: Vec<User> = um.into_values().collect();
        let res = serde_json::to_string(&us)?;
        f.write_all(res.as_bytes())?;
        Ok(())
    }

    fn add(self: &mut Self, user: &User) -> Result<(), ChatError> {
        if self.users.contains_key(&user.name) {
            return Err(Box::new(std::io::Error::new(io::ErrorKind::Other, "ERROR: This user already exists.")));
        } else {
            self.users.insert(user.name.clone(), user.clone());
            self.save()
        }

    }
}

#[get("/app")]
fn app_channel(user_manager: &State<Context>, ws: ws::WebSocket) -> ws::Channel<'static> {
    use rocket::futures::{SinkExt, StreamExt};
    let user_manager = user_manager.inner().clone();
    ws.channel(move |mut stream| Box::pin(async move {
        let mut cur: Option<String> = None;
        let (mut sender,mut receiver) = unbounded::<String>();
        loop {
            select! {
                message = stream.next() => {
                    if message.is_none() {
                        println!("END");
                        break;
                    }
                    if let Some(Ok(ws::Message::Text(message))) = message {
                        println!("Got message: {}", message);
                        if let Some(cmd) = parseCommand(message.as_str()) {
                            let mut user_manager = user_manager.lock().await;
                            match cmd {
                                Command::Register{
                                    name,
                                    password
                                } => {
                                    if user_manager.add(&User{name, password}).is_err() {
                                        let _ = stream.send(ws::Message::Text("Registration error".to_string())).await;
                                    } else {
                                        let _ = stream.send(ws::Message::Text("Registration successful".to_string())).await;
                                    }
                                },
                                Command::Login{name, password} => {
                                    if let Some(pass) = user_manager.users.get(name.as_str()) {
                                        if pass.password == password {
                                            cur = Some(name.clone());
                                            user_manager.onlines.insert(name, sender.clone());
                                            let _ = stream.send(ws::Message::Text("Login Successful".to_string())).await;
                                        } else {
                                            let _ = stream.send(ws::Message::Text("Wrong Password".to_string())).await;

                                        }
                                    } else {
                                            let _ = stream.send(ws::Message::Text("User does not exist".to_string())).await;
                                    }
                                },
                                Command::Text{name, content} => {
                                    if let  Some(ref cur_name) = cur {
                                        if let Some(s) = user_manager.onlines.get_mut(name.as_str()) {
                                            let msg = format!("From {}: {}", cur_name, content.as_str());
                                            let _ = s.send(msg).await;
                                        } else {
                                                let _ = stream.send(ws::Message::Text("User is not online".to_string())).await;
                                        }
                                    } else {
                                                let _ = stream.send(ws::Message::Text("Login first".to_string())).await;
                                    }
                                },
                                Command::Quit => {
                                    break;
                                }
                                
                            }
                        } else {
                            let _ = stream.send(ws::Message::Text("Unknown command".to_string())).await;
                        }


                    } else {
                        println!("DEBUG_END2");
                        break;
                    }
                },
                content = receiver.next()  => {
                    if content.is_some() {
                        let _ = stream.send(ws::Message::Text(content.unwrap())).await;
                    }
                }
            }        
        }
        println!("DEBUG_END_NEXT");
        if let Some(name) = cur {
            let mut user_manager = user_manager.lock().await;
            user_manager.onlines.remove(name.as_str());
            println!("User {} has quited.", name);
        }
        Ok(())
    }))
}

type Context = Arc<Mutex<UserManager>>;

#[launch]
fn rocket () -> _ {

    let user_manager = UserManager::load("users.json").unwrap();

    let figment = rocket::Config::figment().merge(("port", 1111));

    rocket::custom(figment).manage(Arc::new(Mutex::new(user_manager))).mount("/", routes![app_channel])
}