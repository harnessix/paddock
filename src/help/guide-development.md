## Development

```sh
bin/test
```

To contribute, read [CONTRIBUTING.md](CONTRIBUTING.md). `bin/test` runs the unit tests, the integration tests, the leak guard, and `paddock doctor --escape` on each backend that the host can run. One test makes sure that tracked files contain no facts about the development machine: the home directory, the user name, the host name, the git server, or the git email.
