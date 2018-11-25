extern crate dockworker;

use std::io::prelude::*;
use std::io::{BufReader};
use std::io::Error;
use std::ops::Fn;
use std::clone::Clone;
use std::time::Duration;
use std::fmt;
use dockworker::*;

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

fn main() {
    let image_name = "myimage:latest";
    let command_line = vec!["cargo".to_string(), "build".to_string()];
    let results = try_do(image_name, &command_line);

    println!("Results of running {:?}", &command_line);

    match results {
        Ok(transitions) => {
            let mut is_first = true;

            for transition in transitions {
                if is_first {
                    if let Some(before) = transition.before {
                        println!("{} -- {}", before.result, truncate(
                            &before.layer.creation_command, 100));
                    }
                }
                println!("{} -- {}", transition.after.result, truncate(
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

fn get_changes(layers: Vec<Layer>, action: &Fn(&str) -> String)
               -> Result<Vec<Transition>, Error> {
    let first_image_name = &layers.first().unwrap().image_name;
    let last_image_name = &layers.last().unwrap().image_name;
    let start = action(first_image_name);
    let end = action(last_image_name);

    if start == end {
        return Ok(vec![
            Transition {
                before: None,
                after: LayerResult { layer: layers.last().unwrap().clone(), result: start },
            }
        ]);
    }

    bisect(&layers[1..layers.len() - 1],
           &LayerResult { layer: layers.first().unwrap().clone(), result: start },
           &LayerResult { layer: layers.last().unwrap().clone(), result: end },
           &action)
}

fn bisect(history: &[Layer], start: &LayerResult, end: &LayerResult, action: &Fn(&str) -> String)
          -> Result<Vec<Transition>, Error> {
    let mut results: Vec<Transition> = Vec::new();
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
    let mid_result = LayerResult { layer: history[half].clone(), result: action(&history[half].image_name) };

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
            return bisect(&history[half + 1..], &mid_result, &end, &action);
        }
    if mid_result.result == end.result
        {
            return bisect(&history[..half], &start, &mid_result, &action);
        }

    let left = bisect(&history[..half], &start, &mid_result, &action).unwrap();
    let right = bisect(&history[half + 1..], &mid_result, &end, &action).unwrap();

    results.extend(left);
    results.extend(right);
    Ok(results)
}

fn try_do(image_name: &str, command_line: &Vec<String>) -> Result<Vec<Transition>, Error> {
    let docker = Docker::connect_with_defaults().unwrap();

    let create_and_try_container = |container_id: &str| -> String
        {
            println!(".");
            let timeout_in_seconds = 2u64;
            //Remove any existing container with same name...
            let container_name = String::from("AA") + &(std::time::SystemTime::now().elapsed().unwrap()).as_secs().to_string();
            //println!("CONT name : {}", &container_name);
            let _result = docker.remove_container(&container_name, None, Some(true), None);

            //Create container
            let mut create = ContainerCreateOptions::new(&container_id);
            let mut host_config = ContainerHostConfig::new();
            host_config.auto_remove(false);
            create.host_config(host_config);
            let it = command_line.iter();
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
        };

    let histories = docker.history_image(image_name).unwrap();

    let mut layers = Vec::new();
    for event in histories.iter() { //TODO assert only one history.
        match event.id.clone() {
            Some(layer_name) => {
                layers.push(
                    Layer {
                        image_name: layer_name,
                        creation_command: event.created_by.clone(),
                    })
            }
            None => println!("Skipping {} - no layer found.", event.created_by)
        }
    }
    get_changes(layers, &create_and_try_container)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(id: &str) -> Layer {
        Layer { image_name: id.to_string(), creation_command: id.to_string() }
    }

    #[test]
    fn if_output_always_same_return_earliest_command() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3")], &|x| match x {
            "1" => "A".to_string(),
            "2" => "A".to_string(),
            "3" => "A".to_string(),
            _ => panic!("unhandled {}", x)
        });

        assert_eq!(results.unwrap(), vec![
            Transition {
                before: None,
                after: LayerResult { layer: lay("3"), result: "A".to_string() },
            }
        ]);
    }

    #[test]
    fn if_one_difference_show_command_that_made_difference() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3")], &|x| match x {
            "1" => "A".to_string(),
            "2" => "A".to_string(),
            "3" => "B".to_string(),
            _ => panic!("unhandled {}", x)
        });

        assert_eq!(results.unwrap(), vec![
            Transition {
                before: Some(LayerResult { layer: lay("2"), result: "A".to_string() }),
                after: LayerResult { layer: lay("3"), result: "B".to_string() },
            }
        ]);
    }

    #[test]
    fn if_two_differences_show_two_commands_that_made_difference() {
        let results = get_changes(vec![lay("1"), lay("2"), lay("3"), lay("4")], &|x| match x {
            "1" => "A".to_string(),
            "2" => "B".to_string(),
            "3" => "B".to_string(),
            "4" => "C".to_string(),
            _ => panic!("unhandled {}", x)
        });

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
        let counter = std::rc::Rc::new(Box::new(0u32));

        let results = get_changes(vec![lay("1"), lay("2"), lay("3"), lay("4")
                                       , lay("5"), lay("6"), lay("7"), lay("8"), lay("9"), lay("10")
        ], &|x| {
            match x {
                "1" => "A".to_string(),
                "2" => "B".to_string(),
                "3" => "B".to_string(),
                "4" => "C".to_string(),
                "5" => "C".to_string(),
                "6" => "C".to_string(),
                "7" => "C".to_string(),
                "8" => "C".to_string(),
                "9" => "D".to_string(),
                "10" => "D".to_string(),
                _ => panic!("unhandled {}", x)
            }
        });

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