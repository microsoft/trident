from invoke.watchers import StreamWatcher

LOCAL_TRIDENT_CONFIG_PATH = "/etc/trident/config.yaml"
TRIDENT_EXECUTABLE_PATH = "/usr/bin/trident"


class OutputWatcher(StreamWatcher):
    def __init__(self):
        super().__init__()
        self.output_len = 0

    def submit(self, stream):
        new_output = stream[self.output_len :]
        print(new_output, end="")
        self.output_len = len(stream)
        return []


def run_ssh_command(connection, command):
    """
    Runs a command on the host using Fabric and returns the combined stdout and
    stderr.
    """
    try:
        # Executes a command using Fabric and returns the result
        result = connection.run(command, warn=True, hide="both")
        # Combining stdout and stderr for compatibility with the original function's return
        return result.stdout + result.stderr
    except Exception as e:
        print(f"An unexpected error occurred:\n")
        raise
