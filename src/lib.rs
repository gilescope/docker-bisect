//! # docker-bisect
//! `docker-bisect` create assumes that the docker daemon is running and that you have a
//! docker image with cached layers to probe.
extern crate colored;
extern crate dockworker;
extern crate indicatif;
extern crate rand;

use std::clone::Clone;
use std::fmt;
use std::io::{prelude::*, BufReader, Error};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use colored::*;
use dockworker::*;
use indicatif::ProgressBar;
use rand::Rng;

/// Truncates a string to a single line with a max width
/// and removes docker prefixes.
///
/// # Example
/// ```
/// use docker_bisect::truncate;
/// let line = "blar #(nop) real command\n line 2";
/// assert_eq!("real com", truncate(&line, 8));
/// ```
pub fn truncate(mut s: &str, max_chars: usize) -> &str {
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

/// A layer in a docker image. (A layer is a set of files changed due to the previous command).
#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
pub struct Layer {
    pub height: usize,
    pub image_name: String,
    pub creation_command: String,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {:?}", self.image_name, self.creation_command)
    }
}

/// The stderr/stdout of running the command on a container made of this layer
/// (on top of all earlier layers). If command hit the timeout the result may be truncated or empty.
#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq)]
pub struct LayerResult {
    pub layer: Layer,
    pub result: String,
}

impl fmt::Display for LayerResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {}", self.layer, self.result)
    }
}

/// A Transition is the LayerResult of running the command on the lower layer
/// and of running the command on the higher layer. No-op transitions are not recorded.
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq)]
pub struct Transition {
    pub before: Option<LayerResult>,
    pub after: LayerResult,
}

impl fmt::Display for Transition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.before {
            Some(be) => write!(f, "({} -> {})", be, self.after),
            None => write!(f, "-> {}", self.after),
        }
    }
}

/// Starts the bisect operation. Calculates highest and lowest layer result and if they have
/// different outputs it starts a binary chop to figure out which layer(s) caused the change.
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
    command_line: Vec<String>,
    timeout_in_seconds: usize,
}

impl DockerContainer {
    fn new(total: u64, command_line: Vec<String>, timeout_in_seconds: usize) -> DockerContainer {
        let pb = Arc::new(ProgressBar::new(total));

        DockerContainer {
            pb,
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

        let timeout = Duration::from_secs(self.timeout_in_seconds as u64);
        //TODO stop sleeping if container is finished.
        std::thread::sleep(timeout);
        self.pb.inc(1);

        let mut container_output = String::new();

        let result = docker.log_container(&container_name, &log_options);
        if let Ok(result) = result {
            let mut line_reader = BufReader::new(result);
            let _size = line_reader.read_to_string(&mut container_output);
        }
        //TODO stop could be async...
        let _stop_result = docker.stop_container(&container.id, timeout);
        container_output
    }

    fn skip(&self, count: u64) -> () {
        self.pb.inc(count);
    }
}

/// Struct to hold parameters.
pub struct BisectOptions {
    pub timeout_in_seconds: usize,
    pub trunc_size: usize,
}

/// Create containers based on layers and run command_line against them.
/// Result is the differences in std out and std err.
pub fn try_bisect(
    histories: &Vec<ImageLayer>,
    command_line: Vec<String>,
    options: BisectOptions,
) -> Result<Vec<Transition>, Error> {
    println!(
        "\n{}\n\n{:?}\n",
        "Command to apply to layers:".bold(),
        &command_line
    );
    let create_and_try_container = DockerContainer::new(
        histories.len() as u64,
        command_line,
        options.timeout_in_seconds,
    );

    println!("{}", "Skipped missing layers:".bold());
    println!();

    let mut layers = Vec::new();
    for (index, event) in histories.iter().rev().enumerate() {
        let mut created = event.created_by.clone();
        created = truncate(&created, options.trunc_size).to_string();
        match event.id.clone() {
            Some(layer_name) => layers.push(Layer {
                height: index,
                image_name: layer_name,
                creation_command: event.created_by.clone(),
            }),
            None => println!("{:<3}: {}.", index, truncate(&created, options.trunc_size)),
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
        return Err(Error::new(
            std::io::ErrorKind::Other,
            "no cached layers found!",
        ));
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
