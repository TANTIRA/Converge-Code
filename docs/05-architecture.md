# Architecture

One session has surfaces, a kernel, loops, and a repo. MMDP appears when the session leaves the laptop.

## Context

```mermaid
flowchart TB
    subgraph Surfaces[Surfaces]
        TTY[Terminal]
        IDE[IDE client]
        P[converge -p and outer triggers]
    end

    UHP[UHP: session, stream, cancel, tasks]
    Kernel[Ontology kernel]
    Mem[Memory]
    Loop[Inner loop]
    Outer[Outer loop]
    Perm[Permission mode and hooks]

    subgraph Children[Children]
        One[Single agent]
        Async[Async worktree]
        Dyn[Workflow agent parallel pipeline]
    end

    Repo[Git repo]
    Model[Model adapter]
    MMDP[MMDP off the machine]

    Surfaces --> UHP
    UHP --> Kernel
    Mem --> Kernel
    Perm --> Kernel
    Kernel --> Loop
    Outer --> Loop
    Loop --> Model
    Loop --> Repo
    Loop --> One
    Loop --> Async
    Loop --> Dyn
    Async --> Repo
    Dyn --> Repo
    Outer --> MMDP
    Kernel --> MMDP
```

## End-to-end flow

```mermaid
sequenceDiagram
    participant U as User
    participant C as Terminal or IDE
    participant K as Kernel
    participant L as Loop
    participant M as Model
    participant R as Repo
    participant A as Child

    U->>C: Chat, selection, or -p
    C->>K: session_id, mode, input
    K->>L: Memory, goal, budget, admitted tools
    L->>M: Turn with the goal inserted
    M->>R: Edit or command, according to OS and mode
    L->>L: Hook and test command
    L->>A: Worktree when the mode admits spawn
    A-->>L: Result arrives later
    L->>K: Completion command exits 0
    K->>R: Lesson
    K-->>C: Stream, diff, goal status
```

The outer loop uses the same flow when the trigger is a schedule or a git event. An outer round may be a new process with an empty context. Progress is read from git and the session store. The verifier is the test command or a child that only reads.

## Planes

| Plane | Role for the CLI and IDE |
|---|---|
| UHP | Session, stream to the terminal and clients, cancel, resume, `/tasks` |
| A2A | Delegation. A message carries the goal and the worktree path, not file contents |
| MCP | External tools. Schemas of servers that are not in use stay out of the initial context |
| MMDP | Data, compute, and the model catalog when the session leaves the laptop |
| Kernel | Schema, functions, actions |
| Security | Permission decisions, hooks, budget, lineage. Bytes do not pass through here |

Contract details are in [10-planes.md](10-planes.md).
