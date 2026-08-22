# Documentation rules

ATLANTIS uses progressive disclosure. Each reader starts with a small index and opens detailed documents only when necessary.

## Document locations

| Location     | Content                                                             |
| ------------ | ------------------------------------------------------------------- |
| `README.md`  | Gives the project purpose and the three documentation entry points. |
| `docs/user`  | Gives setup, use, query, and operation procedures.                  |
| `docs/code`  | Gives architecture, contracts, and development procedures.          |
| `docs/agent` | Keeps generated plans separate from current instructions.           |

Do not put current setup instructions in an agent document. Agent documents can become obsolete.

## Language standard

Use [ASD-STE100 Simplified Technical English, Issue 9](https://www.asd-ste100.org/assets/files/ASD-STE100_ISSUE9.pdf) as the writing source.

These project rules summarize the principal controls. The official standard remains authoritative.

### Procedures

- Use a maximum of 20 words in each instruction.
- Give one instruction in each sentence.
- Start an instruction with an imperative verb.
- Put a required condition before the instruction.
- Keep information out of procedure notes when the reader needs it for the task.

### Descriptions

- Use a maximum of 25 words in each sentence.
- Give one topic in each sentence.
- Give information gradually.
- Use a maximum of six sentences in each paragraph.

### General text

- Use active voice when possible.
- Use one term for one meaning.
- Use project identifiers as technical nouns.
- Use vertical lists for complex information.
- Do not use contractions.
- Do not use semicolons.
- Use American English spelling.

The project does not use an automated STE certification tool. Review technical accuracy and language accuracy during each documentation change.

## Links and commands

Use relative links between repository documents. Check each moved document for incoming and outgoing links.

Use commands that exist in `package.json` or repository scripts. Show the working directory when a command depends on it.

Do not show placeholder commands as a complete quick start. State required data and external services before the command.
