# Docker-Bisect

Docker-Bisect is inspired by [git-bisect][https://git-scm.com/docs/git-bisect].

The tool will inspect the layets that make up a docker image and for each layer run the same command. It will report which layers caused the command to have a different output.

## Usage

```
Docker Bisect 0.1
Giles Cope <gilescope@gmail.com>
Run a command against image layers, find which layers change the output.

USAGE:
    docker-bisect [FLAGS] <image_name> <command>...

FLAGS:
    -h, --help        Prints help information
    -t, --timeout     Number of seconds to run each command for
        --truncate    Number of chars to truncate to (default is term width)
    -V, --version     Prints version information

ARGS:
    <image_name>    Docker image name or id to use
    <command>...    Command and args to call in the container
```

## License

Same duel license as Rust: You can use Apache or MIT.
