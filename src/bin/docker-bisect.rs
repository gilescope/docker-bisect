extern crate clap;
extern crate colored;
extern crate docker_bisect;
extern crate dockworker;
extern crate terminal_size;

use std::io::Error;

use clap::{App, Arg};
use colored::*;
use docker_bisect::*;
use dockworker::*;
use terminal_size::{terminal_size, Width};

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
                .help("Max width of printed layer commands (default is term width)"),
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

    let results: Result<Vec<Transition>, Error> = try_bisect(
        &histories,
        command_line,
        BisectOptions {
            timeout_in_seconds: matches
                .value_of("timeout")
                .unwrap_or("10")
                .parse()
                .expect("Can't parse timeout value, expected --timeout=10 "),
            trunc_size,
        },
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
        Err(e) => {
            println!("{:?}", e);
            std::process::exit(-1);
        }
    }
    //print any training steps...
    if printed_height < histories.len() {
        for (i, layer) in histories.iter().rev().enumerate().skip(printed_height + 1) {
            println!("{}: {}", i, truncate(&layer.created_by, trunc_size).bold());
        }
    }
}
