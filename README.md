# Personal Knowledge Management Tool
This CLI tool can be sued to interact with different markdown flavours that are used for PKM. At the moment, LogSeq, Obsidian and zk are supported.
At the moment, the main function is to import data from Todoist into a format that works with your note taking system.
By using this tool, you can use Todoist as an intermediate storage for urls that are than later imported into your note taking system. The created data is tagged automatically based on configurable keyword-based rules (see below).
The exact way the data is represented depends on the system:
- In LogSeq, a new block representing the data is added to the journal
- In zk, the data is stored in a new file and a link to that file is added to the daily journal
- Obsidian is yet to be implemented

## Todoist Import
For this to work, you need to setup API keys (see below).
Once this is done, you can run `pkmt todoi --help` to see the available commands (assuming you have built this tool using e.g. `cargo build --release` and made the generated binary available in path).
You can choose via a flag whether the corresponding todoist tasks should be marked as completed.

At the moment, the import procedure considers only todoist inbox tasks that are not scheduled and don't have any sub-tasks.
There are specialized import functions for YouTube and Stronger By Science (requiring template files with fitting names). For other urls, you are asked which template to use. The chosen template gets populated with the url and keyword-based tags.

You can use `pkmt todoi-config` (and the associated sub-commands) to change the config, e.g. to add more keywords.

## Setup
### Todoi
Place keys file at `~/.config/pkmt/keys.txt`
Contents:
```
yt_api_key = "..."
todoist_api_key = "..."
```

## Goals
- convert between different formats
