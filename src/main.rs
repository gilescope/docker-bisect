extern crate clap;
extern crate colored;
extern crate dockworker;
extern crate indicatif;
extern crate rand;
extern crate terminal_size;

use std::clone::Clone;
use std::fmt;
use std::io::{prelude::*, BufReader, Error};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::{App, Arg};
use colored::*;
use dockworker::*;
use indicatif::ProgressBar;
use rand::Rng;
use terminal_size::{terminal_size, Width};

fn truncate(mut s: &str, max_chars: usize) -> &str {
    s = s.lines().next().expect("nothing to truncate");
    if s.contains("#(nop) ") {
        let mut splat = s.split(" #(nop) ");
        let _ = splat.next();
        s = splat.next().expect("#(nop) with no command in.");
        s = s.trim();
    }
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

fn main() {
    let matches = App::new("docker-bisect")
        .version("0.1")
        .about("Run a command against image layers, find which layers change the output.")
        .arg(
            Arg::with_name("timeout")
                .short("t")
                .long("timeout")
                .help("Number of seconds to run each command for"),
        ).arg(
            Arg::with_name("image")
                .value_name("image_name")
                .help("Docker image name or id to use")
                .required(true)
                .takes_value(true),
        ).arg(
            Arg::with_name("command")
                .help("Command and args to call in the container")
                .required(true)
                .multiple(true),
        ).arg(
        Arg::with_name("truncate")
            .long("truncate")
            .help("Number of chars to truncate to (default is term width)"),
        ).get_matches();

    let image_name = matches.value_of("image").expect("image expected");
    let mut command_line = Vec::<String>::new();

    for arg in matches.values_of("command").expect("command expected") {
        command_line.push(arg.to_string());
    }

    let mut trunc_size: usize = matches
        .value_of("truncate")
        .unwrap_or("100")
        .parse()
        .expect("Can't parse timeout value, expected --timeout=10 ");

    let size = terminal_size();
    if let Some((Width(w), _)) = size {
        if trunc_size == 100 {
            trunc_size = (w as usize) - 10;
        }
    }

    let docker: Docker =
        Docker::connect_with_defaults().expect("Can't connect to docker daemon. Is it running?");

    let histories: Vec<ImageLayer> = docker
        .history_image(image_name)
        .expect("Can't get layers from image.");

    let results: Result<Vec<Transition>, Error> = try_do(
        &histories,
        image_name,
        command_line,
        matches
            .value_of("timeout")
            .unwrap_or("10")
            .parse()
            .expect("Can't parse timeout value, expected --timeout=10 "),
        trunc_size,
    );

    println!();
    println!("{}", "\nResults ==>".bold());
    println!();

    let mut printed_height = 0;
    match results {
        Ok(mut transitions) => {
            transitions.sort_by(|t1, t2| t1.after.layer.height.cmp(&t2.after.layer.height));

            for transition in transitions {
                //print previous steps...
                if printed_height < transition.after.layer.height {
                    for (i, layer) in histories
                        .iter()
                        .rev()
                        .enumerate()
                        .skip(printed_height + 1)
                        .take(transition.after.layer.height - (printed_height + 1))
                    {
                        println!("{}: {}", i, truncate(&layer.created_by, trunc_size).bold());
                    }
                }

                println!(
                    "{}: {} CAUSED:\n\n {}",
                    transition.after.layer.height,
                    truncate(&transition.after.layer.creation_command, trunc_size).bold(),
                    transition.after.result
                );
                printed_height = transition.after.layer.height;
            }
        }
        Err(e) => println!("{:?}", e),
    }
    //print any training steps...
    if printed_height < histories.len() {
        for (i, layer) in histories.iter().rev().enumerate().skip(printed_height + 1) {
            println!("{}: {}", i, truncate(&layer.created_by, trunc_size).bold());
        }
    }
}

#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
struct Layer {
    height: usize,
    image_name: String,
    creation_command: String,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {:?}", self.image_name, self.creation_command)
    }
}

#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
struct LayerResult {
    layer: Layer,
    result: String,
}

impl fmt::Display for LayerResult {
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
            Some(be) => write!(f, "({} -> {})", be, self.after),
            None => write!(f, "-> {}", self.after),
        }
    }
}

fn get_changes<T>(layers: Vec<Layer>, action: &T) -> Result<Vec<Transition>, Error>
where
    T: ContainerAction + 'static,
{
    let first_layer = layers.first().expect("no first layer");
    let last_layer = layers.last().expect("no last layer");

    let first_image_name: String = first_layer.image_name.clone();
    let last_image_name = &last_layer.image_name;

    let action_c = action.clone();
    let left_handle = thread::spawn(move || action_c.try_container(&first_image_name));

    let end = action.try_container(last_image_name);
    let start = left_handle.join().expect("first layer execution error!");

    if start == end {
        return Ok(vec![Transition {
            before: None,
            after: LayerResult {
                layer: last_layer.clone(),
                result: start,
            },
        }]);
    }

    bisect(
        Vec::from(&layers[1..layers.len() - 1]),
        LayerResult {
            layer: first_layer.clone(),
            result: start,
        },
        LayerResult {
            layer: last_layer.clone(),
            result: end,
        },
        action,
    )
}

fn bisect<T>(
    history: Vec<Layer>,
    start: LayerResult,
    end: LayerResult,
    action: &T,
) -> Result<Vec<Transition>, Error>
where
    T: ContainerAction + 'static,
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
            results.push(Transition {
                before: Some(start.clone()),
                after: mid_result.clone(),
            });
        }
        if mid_result.result != *end.result {
            results.push(Transition {
                before: Some(mid_result),
                after: end.clone(),
            });
        }
        return Ok(results);
    }

    if start.result == mid_result.result {
        action.skip((mid_result.layer.height - start.layer.height) as u64);
        return bisect(Vec::from(&history[half + 1..]), mid_result, end, action);
    }
    if mid_result.result == end.result {
        action.skip((end.layer.height - mid_result.layer.height) as u64);
        return bisect(Vec::from(&history[..half]), start, mid_result, action);
    }

    let clone_a = action.clone();
    let clone_b = action.clone();
    let mid_result_c = mid_result.clone();

    let hist_a = Vec::from(&history[..half]);

    let left_handle = thread::spawn(move || bisect(hist_a, start, mid_result, &clone_a));
    let right_handle =
        thread::spawn(move || bisect(Vec::from(&history[half + 1..]), mid_result_c, end, &clone_b));
    let mut left_results: Vec<Transition> = left_handle
        .join()
        .expect("left")
        .expect("left transition err");

    let right_results: Vec<Transition> = right_handle
        .join()
        .expect("right")
        .expect("right transition err");

    left_results.extend(right_results); // These results are sorted later...
    Ok(left_results)
}

trait ContainerAction: Clone + Send {
    fn try_container(&self, container_id: &str) -> String;
    fn skip(&self, count: u64) -> ();
}

#[derive(Clone)]
struct DockerContainer {
    pb: Arc<ProgressBar>,
    image_name: String,
    command_line: Vec<String>,
    timeout_in_seconds: u64,
}

impl DockerContainer {
    fn new(
        total: u64,
        image_name: String,
        command_line: Vec<String>,
        timeout_in_seconds: u64,
    ) -> DockerContainer {
        let pb = Arc::new(ProgressBar::new(total));

        DockerContainer {
            pb,
            image_name,
            command_line,
            timeout_in_seconds,
        }
    }
}

impl ContainerAction for DockerContainer {
    fn try_container(&self, container_id: &str) -> String {
        let docker: Docker = Docker::connect_with_defaults().expect("docker daemon running?");
        let container_name: String = rand::thread_rng().gen_range(0., 1.3e4).to_string();

        //Create container
        let mut create = ContainerCreateOptions::new(&container_id);
        let mut host_config = ContainerHostConfig::new();
        host_config.auto_remove(false);
        create.host_config(host_config);
        let it = self.command_line.iter();
        for command in it {
            create.cmd(command.clone());
        }

        let container: CreateContainerResponse = docker
            .create_container(Some(&container_name), &create)
            .expect("couldn't create container");

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
            follow: true,
        };

        std::thread::sleep(Duration::from_secs(self.timeout_in_seconds));
        self.pb.inc(1);

        let mut container_output = String::new();

        let result = docker.log_container(&container_name, &log_options);
        if let Ok(result) = result {
            let mut line_reader = BufReader::new(result);
            let _size = line_reader.read_to_string(&mut container_output);
        }
        let _stop_result =
            docker.stop_container(&container.id, Duration::from_secs(self.timeout_in_seconds));
        container_output
    }

    fn skip(&self, count: u64) -> () {
        self.pb.inc(count);
    }
}

fn try_do(
    histories: &Vec<ImageLayer>,
    image_name: &str,
    command_line: Vec<String>,
    timeout_in_seconds: u64,
    trunk_size: usize,
) -> Result<Vec<Transition>, Error> {
    println!(
        "\n{}\n\n{:?}\n",
        "Command to apply to layers:".bold(),
        &command_line
    );
    let create_and_try_container = DockerContainer::new(
        histories.len() as u64,
        String::from(image_name),
        command_line,
        timeout_in_seconds,
    );

    println!("{}", "Skipped missing layers:".bold());
    println!();

    let mut layers = Vec::new();
    for (index, event) in histories.iter().rev().enumerate() {
        let mut created = event.created_by.clone();
        created = truncate(&created, trunk_size).to_string();
        match event.id.clone() {
            Some(layer_name) => layers.push(Layer {
                height: index,
                image_name: layer_name,
                creation_command: event.created_by.clone(),
            }),
            None => println!("{:<3}: {}.", index, truncate(&created, trunk_size)),
        }
    }

    println!();
    println!(
        "{}",
        "Bisecting found layers (running command on the layers) ==>\n".bold()
    );

    if layers.len() < 2 {
        println!();
        eprintln!(
            "{} layers found in cache - not enough layers to bisect.",
            layers.len()
        );
        std::process::exit(-1);
    }

    let results = get_changes(layers, &create_and_try_container);
    create_and_try_container.pb.finish_with_message("done");
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone)]
    struct MapAction {
        map: HashMap<String, String>,
    }

    impl MapAction {
        fn new(from: Vec<usize>, to: Vec<&str>) -> Self {
            let mut object = MapAction {
                map: HashMap::new(),
            };
            for (f, t) in from.iter().zip(to.iter()) {
                object.map.insert(f.to_string(), t.to_string());
            }
            object
        }
    }

    impl ContainerAction for MapAction {
        fn try_container(&self, container_id: &str) -> String {
            let none = String::new();
            let result: &String = self.map.get(container_id).unwrap_or(&none);
            result.clone()
        }

        fn skip(&self, _count: u64) -> () {}
    }

    fn lay(id: usize) -> Layer {
        Layer {
            height: id,
            image_name: id.to_string(),
            creation_command: id.to_string(),
        }
    }

    #[test]
    fn if_output_always_same_return_earliest_command() {
        let results = get_changes(
            vec![lay(1), lay(2), lay(3)],
            &MapAction::new(vec![1, 2, 3], vec!["A", "A", "A"]),
        );

        assert_eq!(
            results.unwrap(),
            vec![Transition {
                before: None,
                after: LayerResult {
                    layer: lay(3),
                    result: "A".to_string()
                },
            }]
        );
    }

    #[test]
    fn if_one_difference_show_command_that_made_difference() {
        let results = get_changes(
            vec![lay(1), lay(2), lay(3)],
            &MapAction::new(vec![1, 2, 3], vec!["A", "A", "B"]),
        );

        assert_eq!(
            results.unwrap(),
            vec![Transition {
                before: Some(LayerResult {
                    layer: lay(2),
                    result: "A".to_string()
                }),
                after: LayerResult {
                    layer: lay(3),
                    result: "B".to_string()
                },
            }]
        );
    }

    #[test]
    fn if_two_differences_show_two_commands_that_made_difference() {
        let results = get_changes(
            vec![lay(1), lay(2), lay(3), lay(4)],
            &MapAction::new(vec![1, 2, 3, 4], vec!["A", "B", "B", "C"]),
        );

        let res = results.unwrap();

        assert_eq!(
            res,
            vec![
                Transition {
                    before: Some(LayerResult {
                        layer: lay(1),
                        result: "A".to_string()
                    }),
                    after: LayerResult {
                        layer: lay(2),
                        result: "B".to_string()
                    },
                },
                Transition {
                    before: Some(LayerResult {
                        layer: lay(3),
                        result: "B".to_string()
                    }),
                    after: LayerResult {
                        layer: lay(4),
                        result: "C".to_string()
                    },
                }
            ]
        );
    }

    #[test]
    fn three_transitions() {
        let results = get_changes(
            vec![
                lay(1),
                lay(2),
                lay(3),
                lay(4),
                lay(5),
                lay(6),
                lay(7),
                lay(8),
                lay(9),
                lay(10),
            ],
            &MapAction::new(
                vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
                vec!["A", "B", "B", "C", "C", "C", "C", "C", "D", "D"],
            ),
        );
        let res = results.unwrap();

        assert_eq!(
            res,
            vec![
                Transition {
                    before: Some(LayerResult {
                        layer: lay(1),
                        result: "A".to_string()
                    }),
                    after: LayerResult {
                        layer: lay(2),
                        result: "B".to_string()
                    },
                },
                Transition {
                    before: Some(LayerResult {
                        layer: lay(3),
                        result: "B".to_string()
                    }),
                    after: LayerResult {
                        layer: lay(4),
                        result: "C".to_string()
                    },
                },
                Transition {
                    before: Some(LayerResult {
                        layer: lay(8),
                        result: "C".to_string()
                    }),
                    after: LayerResult {
                        layer: lay(9),
                        result: "D".to_string()
                    },
                }
            ]
        );
    }
}
