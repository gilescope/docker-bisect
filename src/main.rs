extern crate dockworker;

use std::io::prelude::*;
use std::io::{BufReader, BufRead};
use std::fs::File;
use std::io::Error;
use std::ops::Fn;
use std::clone::Clone;
use std::time::Duration;
use dockworker::*;
use dockworker::container::*;
use std::fmt;

fn main() {
    let command_line = vec!["cargo".to_string(), "build".to_string()];
    try_do(command_line);
}

#[derive(Debug, Clone,Eq, Ord, PartialOrd, PartialEq)]
struct Layer {
    image_name: String,
    creation_command: String,
}

impl fmt::Display for Layer
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {}", self.image_name, self.creation_command)
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
    after: LayerResult
}


impl fmt::Display for Transition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.before {
            Some(be) => {
                write!(f, "({:?} -> {})", self.before, self.after)
                }
            None => write!(f, "-> {}", self.after)
        }
    }
}   

fn get_changes(layers: Vec<Layer>, action: &Fn(&str) -> String)
               -> Result<Vec<Transition>, Error> {
    let start = action(&layers.first().unwrap().image_name);
    let end = action(&layers.last().unwrap().image_name);

    if start == end {
        return Ok(vec![
            Transition {
                before: None,
                after: LayerResult { layer: layers.last().unwrap().clone(), result: start },
            }
        ]);
    }

    bisect(&layers[1..layers.len() - 1],
           &LayerResult{ layer:layers.first().unwrap().clone(), result:start},
           &LayerResult{ layer:layers.last().unwrap().clone(), result:end},
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
        return Ok(vec![Transition{
            before: Some(start.clone()),
            after: end.clone(),
        }]);
    }

    let half = size / 2;
    let mid_result = LayerResult{ layer: history[half].clone(), result: action(&history[half].image_name) };

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
        return bisect(&history[half+1..], &mid_result, &end, &action);
    }
    if mid_result.result == end.result
    {
        return bisect(&history[..half], &start, &mid_result, &action);
    }

    let left = bisect(&history[..half], &start, &mid_result, &action).unwrap();
    let right = bisect(&history[half+1..], &mid_result, &end, &action).unwrap();

    results.extend(left);
    //results.push(Transition{ before: Some((*start).clone()), after: mid_result });
    results.extend(right);
    Ok(results)
}

mod tests {
    use super::*;

    fn lay(id: &str) -> Layer {
        Layer{image_name:id.to_string(), creation_command: id.to_string()}
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
        ], &|x|{
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
            } });

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

fn try_do(command_line: Vec<String>) -> Result<(), Error> {
    let docker = Docker::connect_with_defaults().unwrap();
    //println!("{:#?}", docker.system_info().unwrap());

    //List containers
    let filter = ContainerFilters::new();
    let containers = docker.list_containers(None, None, None, filter).unwrap();

    containers.iter().for_each(|c| {
        docker.stop_container(&c.Id, Duration::from_secs(2)).unwrap();
        println!("ACTION: stopped {:?}", c);
        docker.remove_container(&c.Id, None, None, None).unwrap();
        println!("ACTION: removed {:?}", c);
    });

//    //Create a continaer against the image
//    let mut create = ContainerCreateOptions::new(&image);
//    create.tty(true);
//    create.entrypoint(vec!["/bin/sh".to_string(), "echo Hello world".to_string()]);
//
//    //instantiate container
//    let container: CreateContainerResponse = docker.create_container(
//        Some("my_container_name"), &create).unwrap();


//    let filter = ContainerFilters::new();
//    let containers = docker.list_containers(None, None, None, filter).unwrap();
//    containers.iter().for_each(|c| {
//        println!("QUERY: status {:?}", c.Status);
//
//        let opts = ContainerListOptions::default();
//        let x = docker.filesystem_changes(c);
//        let info = docker.container_info(c).unwrap();
//        for mount in info.Mounts {
//            println!("mount {:?}", mount);
//        }
//        if let Ok(y) = x {
//            for change in y {
//                println!("FOUND file sys changes::: {:#?}", change);
//            }
//        } else {
//            println!("not found any {}", c)
//        }

    let mut current_output: Option<String> = None;
    let mut current_command: Option<String> = None;

    let histories = docker.history_image("myimage:latest");
    for history in histories {
        for event in history {
            //Gradually going back in time....
            println!("happened {:?} tags: {:?}", event.id, event.tags);
            // println!("happened {}", event.created_by);

            if let Some(image) = event.id {
                //Remove any existing container with same name...
                let container_name = String::from("AA") + &(std::time::SystemTime::now().elapsed().unwrap()).as_secs().to_string();
                //println!("CONT name : {}", &container_name);
                docker.remove_container(&container_name, None, Some(true), None);

                //Create container
                let mut create = ContainerCreateOptions::new(&image);
                let mut host_config = ContainerHostConfig::new();
                host_config.auto_remove(false);
                create.host_config(host_config);
                let it = command_line.iter();
                for command in it {
                    create.cmd(command.clone());
                }

                let container: CreateContainerResponse = docker.create_container(
                    Some(&container_name), &create).unwrap();

                docker.start_container(&container.id).unwrap();

                let log_options = ContainerLogOptions {
                    stdout: true,
                    stderr: true,
                    since: None,
                    timestamps: None,
                    tail: None,
                };

                std::thread::sleep(Duration::from_secs(2));

                let mut container_output = String::new();

                let result = docker.log_container_and_follow(&container_name, &log_options);
                if let Ok(result) = result {
                    let mut size = 1;
                    let mut line_reader = BufReader::new(result);

                    while size != 0 {
                        size = line_reader.read_line(&mut container_output).unwrap();
                    }
                }
                //println!("{:?}", &container_output);

                {
                    let expected = current_output.get_or_insert(container_output.clone());

                    if expected != &container_output {
                        println!("{:?} changed to: {:?}", &container_output, &expected);
                        //Interesting...
                        println!("next command: {:?}", &current_command);
                        println!("previous command: {}", &event.created_by);
                    }
                }
                use std::mem; //when match it's a no-op
                mem::replace(&mut current_output, Some(container_output));

                mem::replace(&mut current_command, Some(event.created_by));

                docker.stop_container(&container.id, Duration::from_secs(2));
            }
        }
    }


    let f = File::open("/Users/gilescope/private/strat2/strategy-runner-py/Dockerfile")?;
    let mut reader = BufReader::new(f);

    let mut lines = String::new();

    reader.read_to_string(&mut lines)?;

    let mut docker_file_so_far = String::new();

    for line in lines.lines() {
        docker_file_so_far.push('\n');
        docker_file_so_far.push_str(line);
//        println!("{}", docker_file_so_far);
//        println!("-----------------------");
    }
    Ok(())
}
