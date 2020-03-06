`matrix_server` (name changing soon, I promise) is an server implementation of the
[Matrix](https://matrix.org) protocol in Rust, using the
[actix-web](https://github.com/actix/actix-web) HTTP server, and the Tokio asynchronous I/O
runtime.

More information:
 - [Design Choices](./design_choices.md)
 - [Project Timeline](./timeline.md)

## Motivation
I needed something to do, and I love Rust. When I started this project, the existing matrix servers
were:
 - Synapse, in Python, the canonical and complete but slow and messy implementation from matrix.org
 - Dendrite, in Go, the successor of synapse, in very early stages and rather confusing
 - Ruma, in Rust, fairly complete (barring federation) but using an old and synchronous web 
 framework, and a bit spaghetti

The main problem with all of these is that they were Not Invented Here.

Other than that, synapse was the only one ready for usage in big rooms, and clearly it is more
efficient to start from scratch rather than contribute to an existing project.

Irony aside, I wanted some actual programming experience, having followed Rust and associated
projects for a couple of years.
