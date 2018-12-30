# docker-bisect

[![Build status](https://ci.appveyor.com/api/projects/status/8af521nx5s7u14v3/branch/master?svg=true)](https://ci.appveyor.com/project/gilescope/docker-bisect/branch/master)

docker-bisect is inspired by [git-bisect](https://git-scm.com/docs/git-bisect).

The tool will inspect the layers that make up a docker image. For each layer it will run the same command. It reports which layers caused the command to have a different output.

## Usage

```
docker-bisect 0.1
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

Public Domain: You can use Unlicense or MIT.
