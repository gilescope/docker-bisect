extern crate dockworker;
extern crate clap;
extern crate rand;

use clap::{Arg, App};
use std::io::prelude::*;
use std::io::BufReader;
use std::io::Error;
use std::clone::Clone;
use std::time::Duration;
use std::fmt;
use std::thread;
use rand::Rng;

use dockworker::*;

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

fn main() {
    let matches = App::new("Docker Bisecter")
        .version("1.0")
        .author("Giles Cope <gilescope@gmail.com>")
        .about("Run a command against image layers, determines which layers change the output.")
        .arg(Arg::with_name("timeout")
                 .short("t")
            .long("timeout")
            .help("Number of seconds to run each command for"))
        .arg(Arg::with_name("image")
            .value_name("image_name")
            .help("Docker image name or id to use")
            .required(true)
            .takes_value(true))
        .arg(Arg::with_name("command")
            .help("Command to call in the container")
            .required(true)
            .multiple(true))
        .get_matches();

    //TODO use timeout setting
    let image_name = matches.value_of("image").unwrap();

    let mut command_line = Vec::<String>::new();

    for arg in matches.values_of("command").unwrap() {
        command_line.push(arg.to_string());
    }

    let results = try_do(image_name, command_line);

    println!();

    match results {
        Ok(transitions) => {
            let mut is_first = true;

            for transition in transitions {
                if is_first {
                    is_first = false;
                    if let Some(before) = transition.before {
                        println!("{} \n {}\n", before.result, truncate(
                            &before.layer.creation_command, 100));
                    }
                }
                println!("{} \n {}\n", transition.after.result, truncate(
                    &transition.after.layer.creation_command, 100));
            }
        }
        Err(e) => println!("{:?}", e)
    }
}

#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
struct Layer {
    image_name: String,
    creation_command: String,
}

impl fmt::Display for Layer
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {:?}", self.image_name, self.creation_command)
    }
}

#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
struct LayerResult {
    layer: Layer,
    result: String,
}

impl fmt::Display for LayerResult
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {}", self.layer, self.result)
    }
}

#[derive(Debug, Eq, Ord, PartialOrd, PartialEq)]
struct Transition {
    before: Option<LayerResult>,
    after: LayerResult,
}

impl fmt::Display for Transition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.before {
            Some(be) => {
                write!(f, "({} -> {})", be, self.after)
            }
            None => write!(f, "-> {}", self.after)
        }
    }
}

fn get_changes<T>(layers: Vec<Layer>, action: &T)
                  -> Result<Vec<Transition>, Error>
    where T: ContainerAction + 'static
{
    let first_image_name: String = layers.first().unwrap().image_name.clone();
    let last_image_name = &layers.last().unwrap().image_name;

    let action_c = action.clone();
    let left_handle = thread::spawn(move || {
        action_c.try_container(&first_image_name)
    });

    let end = action.try_container(last_image_name);
    let start = left_handle.join().unwrap();

    if start == end {
        return Ok(vec![
            Transition {
                before: None,
                after: LayerResult { layer: layers.last().unwrap().clone(), result: start },
            }
        ]);
    }

    bisect(Vec::from(&layers[1..layers.len() - 1]),
           LayerResult { layer: layers.first().unwrap().clone(), result: start },
           LayerResult { layer: layers.last().unwrap().clone(), result: end },
           action)
}

fn bisect<T>(history: Vec<Layer>, start: LayerResult, end: LayerResult, action: &T)
             -> Result<Vec<Transition>, Error>
    where T: ContainerAction + 'static
{
    let size = history.len();
    if size == 0 {
        if start.result == end.result {
            return Err(Error::new(std::io::ErrorKind::Other, ""));
        }
        return Ok(vec![Transition {
            before: Some(start.clone()),
            after: end.clone(),
        }]);
    }

    let half = size / 2;
    let mid_result = LayerResult {
        layer: history[half].clone(),
        result: action.try_container(&history[half].image_name),
    };

    if size == 1 {
        let mut results = Vec::<Transition>::new();
        if *start.result != mid_result.result {
            results.push(
                Transition {
                    before: Some(start.clone()),
                    after: mid_result.clone(),
                }
            );
        }
        if mid_result.result != *end.result {
            results.push(
                Transition {
                    before: Some(mid_result),
                    after: end.clone(),
                }
            );
        }
        return Ok(results);
    }

    if start.result == mid_result.result
        {
            return bisect(Vec::from(&history[half + 1..]), mid_result, end, action);
        }
    if mid_result.result == end.result
        {
            return bisect(Vec::from(&history[..half]), start, mid_result, action);
        }

    let clone_a = action.clone();
    let clone_b = action.clone();
    let mid_result_c = mid_result.clone();

    let hist_a = Vec::from(&history[..half]);

    let left_handle = thread::spawn(move || {
        bisect(hist_a, start,
               mid_result, &clone_a)
    });
    let right_handle = thread::spawn(move || {
        bisect(Vec::from(&history[half + 1..]),
               mid_result_c, end, &clone_b)
    });
    let mut left_results: Vec<Transition> = left_handle.join().unwrap().unwrap();
    let right_results: Vec<Transition> = right_handle.join().unwrap().unwrap();
    left_results.extend(right_results);
    //TODO sort.
    Ok(left_results)
}

trait ContainerAction: Clone + Send {
    fn try_container(&self, container_id: &str) -> String;
}

#[derive(Clone)]
struct DockerContainer {
    image_name: String,
    command_line: Vec<String>,
}

impl ContainerAction for DockerContainer {
    fn try_container(&self, container_id: &'_ str) -> String {
        let docker = Docker::connect_with_defaults().unwrap();
        print!(".");
        let _ = std::io::stdout().flush();

        let timeout_in_seconds = 2u64;
        //Remove any existing container with same name...

        let container_name: String = rand::thread_rng().gen_range(0., 1.3e4).to_string();

//        let mut container_name = String::from(container_id);
//        container_name = container_name.replace(':', "-");
//
//
//        container_name.push_str("-bisect");

//        let _result = docker.remove_container(&container_name, None, Some(true), None);

        //Create container
        let mut create = ContainerCreateOptions::new(&container_id);
        let mut host_config = ContainerHostConfig::new();
        host_config.auto_remove(false);
        create.host_config(host_config);
        let it = self.command_line.iter();
        for command in it {
            create.cmd(command.clone());
        }

        let container: CreateContainerResponse = docker.create_container(
            Some(&container_name), &create).unwrap();

        let result = docker.start_container(&container.id);
        if result.is_err() {
            let err: dockworker::errors::Error = result.unwrap_err();

            return format!("{}", err);
        }

        let log_options = ContainerLogOptions {
            stdout: true,
            stderr: true,
            since: None,
            timestamps: None,
            tail: None,
        };

        std::thread::sleep(Duration::from_secs(timeout_in_seconds));

        let mut container_output = String::new();

        let result = docker.log_container_and_follow(&container_name, &log_options);
        if let Ok(result) = result {
            let mut line_reader = BufReader::new(result);
            let _size = line_reader.read_to_string(&mut container_output);
        }
        let _stop_result = docker.stop_container(&container.id, Duration::from_secs(timeout_in_seconds));
        container_output
    }
}

fn try_do(image_name: &str, command_line: Vec<String>) -> Result<Vec<Transition>, Error> {
    let create_and_try_container = DockerContainer {
        image_name: String::from(image_name),
        command_line: command_line,
    };

    let docker = Docker::connect_with_defaults().unwrap();
    let histories = docker.history_image(image_name).unwrap();

    println!("Image Layers:");
    println!();

    let mut layers = Vec::new();
    for (index, event) in histories.iter().rev().enumerate() { //TODO assert only one history.
        let created = event.created_by.clone().replace("/bin/sh -c #(nop) ", "");
        match event.id.clone() {
            Some(layer_name) => {
                println!("{:<3} Layer   found: {}", index, created);
                layers.push(
                    Layer {
                        image_name: layer_name,
                        creation_command: event.created_by.clone(),
                    })
            }
            None => println!("{:<3} Layer Skipped: {}.", index, created)
        }
    }
    println!();

    if layers.len() < 2 {
        println!();
        eprintln!("{} layers found in cache - not enough layers to bisect.", layers.len());
        std::process::exit(-1);
    }

    get_changes(layers, &create_and_try_container)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone)]
    struct MapAction {
        map: HashMap<String, String>
    }

    impl MapAction {
        fn new(from: Vec<&str>, to: Vec<&str>) -> Self {
            let mut object = MapAction { map: HashMap::new() };
            for (f, t) in from.iter().zip(to.iter()) {
                object.map.insert(f.to_string(), t.to_string());
            }
            object
        }
    }

    impl ContainerAction for MapAction {
        fn try_container(&self, container_id: &'_ str) -> String {
            let none = String::new();
            let result: &String = self.map.get(container_id).unwrap_or(&none);
            result.clone()
        }
    }

    fn lay(id: &str) -> Layer {
        Layer { image_name: id.to_string(), creation_command: id.to_string() }
    }

    #[test]
    fn if_output_always_same_return_earliest_command() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3")],
                                  &MapAction::new(vec!["1", "2", "3"], vec!["A", "A", "A"]));

        assert_eq!(results.unwrap(), vec![
            Transition {
                before: None,
                after: LayerResult { layer: lay("3"), result: "A".to_string() },
            }
        ]);
    }

    #[test]
    fn if_one_difference_show_command_that_made_difference() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3")],
                                  &MapAction::new(vec!["1", "2", "3"], vec!["A", "A", "B"]));

        assert_eq!(results.unwrap(), vec![
            Transition {
                before: Some(LayerResult { layer: lay("2"), result: "A".to_string() }),
                after: LayerResult { layer: lay("3"), result: "B".to_string() },
            }
        ]);
    }

    #[test]
    fn if_two_differences_show_two_commands_that_made_difference() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3"), lay("4")],
                                  &MapAction::new(vec!["1", "2", "3", "4"], vec!["A", "B", "B", "C"]));

        let res = results.unwrap();

        assert_eq!(res, vec![
            Transition {
                before: Some(LayerResult { layer: lay("1"), result: "A".to_string() }),
                after: LayerResult { layer: lay("2"), result: "B".to_string() },
            },
            Transition {
                before: Some(LayerResult { layer: lay("3"), result: "B".to_string() }),
                after: LayerResult { layer: lay("4"), result: "C".to_string() },
            }
        ]);
    }

    #[test]
    fn three_transitions() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3"), lay("4")
                                       , lay("5"), lay("6"), lay("7"), lay("8"), lay("9"), lay("10")],
                                  &MapAction::new(vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
                                                  vec!["A", "B", "B", "C", "C", "C", "C", "C", "D", "D"]));
        let res = results.unwrap();

        assert_eq!(res, vec![
            Transition {
                before: Some(LayerResult { layer: lay("1"), result: "A".to_string() }),
                after: LayerResult { layer: lay("2"), result: "B".to_string() },
            },
            Transition {
                before: Some(LayerResult { layer: lay("3"), result: "B".to_string() }),
                after: LayerResult { layer: lay("4"), result: "C".to_string() },
            },
            Transition {
                before: Some(LayerResult { layer: lay("8"), result: "C".to_string() }),
                after: LayerResult { layer: lay("9"), result: "D".to_string() },
            }
        ]);
    }
}