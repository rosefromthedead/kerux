# Design choices
## Rust
Pros:
 - Fast when used correctly
 - The compiler is so useful - I can write code and be certain it won't have safety issues
 - Perfect balance of "gets out of the way" and feature-richness
 - I have been learning it for a long time

Cons:
 - IDE tooling is young - RLS and RA each fall over on different parts of the project
 - Compilation takes a long time
 - Young; things change quickly

Overall, the speed of Rust makes it suitable for a project like this, aiming to be an alternative
to Synapse which has received criticism for being too slow. Also, the enforced correctness makes it
easier to track things like incomplete or incorrect code.

## Actix
Pros:
 - Concise; the proc macros remove a lot of request parsing boilerplate
 - Fast (apparently)
 - Mature and popular
 - Allows fairly easy custom error types; necessary for Matrix

Cons:
 - Known for unsound usage of unsafe
