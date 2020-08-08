# Project Timeline
## 2019-09-10: Tide, tracing, and inline SQL (1f00a50)
While not the absolute beginning of the project, this is a point before any major architectural
changes or design choices. At this point, it was possible to create an account, log in, and change
one's avatar URL or display name.

The server was based on Tide, a very young web framework with little capacity for boilerplate
removal and the storage backend was PostgreSQL. All SQL statements were written inline in the
Client-Server API endpoints. Also, the server used tracing, an even younger logging library with
support for providing context to events, even those which happen in async contexts.

At this point, I was trying to implement the /client/r0/sync API endpoint which returns events that
have happened in a room. This later turned out to be a bad idea.

## 2019-10-28: Little baby storage API (12dc25e)
At this point, I realised that inline SQL is not very nice. There were duplicated statements all
over the place which made general changes difficult; on top of this I knew I'd want to implement
storage backends which weren't PostgreSQL, and if I didn't do something about the inline SQL this
would have been difficult.

So, I split the SQL off into its own module: `crate::storage::postgres`. This module built on the
`ClientGuard` already exposed by the database connection pool to provide functions which hid the
usage of SQL and could later be implemented in other ways.

## 2019-11-10: Bottom up is better than top down (2d39d72)
While implementing /client/r0/sync I realised I had no idea what I was doing; I was trying to send
clients events that didn't exist yet because it was impossible to create a room or otherwise write
anything to the server. This meant that I didn't know what kind of events to expect and didn't
fully understand certain Matrix concepts, so it was proving difficult to implement /sync.

From here on I decided to implement things in the order they would happen; in other words, not
implement /sync without having some way for clients to create rooms and send messages to them.

## 2019-12-06: Moving to Actix (cc04a17)
Tide was bringing problems; it was moving very quickly and I was struggling to even build the
server. It threatened to move to async-std, which meant I couldn't use tokio-postgres, the only
async Postgres driver (at the time). This meant I had to find a new web framework.

### The search for a new web framework
Having decided to move away from Tide, I had to figure out what to move *to*. A few options were
available, and given that Actix was good enough and much more popular than the rest, I decided it
would be the best option. The popularity meant that Actix had been tested and optimised for a
variety of use cases, and was well-ironed.

## 2019-12-09: Moving to log, from tracing (bf7ca94)
Tracing was very young, and I was failing to make use of its features. Actix and most other
libraries used log instead, so I switched to log in order to simplify the dependency tree and
remove the layers between log and tracing.

## 2020-01-15: StorageExt and event validation (db85e9e)
Event validation and authorisaton is an important part of Matrix, especially for state events which
affect the whole room from sending onwards. I was doing neither of these things, and wanted a clean
way to do both, so in came the StorageExt trait. Originally this was going to be a
`fn validate(&Event) -> bool` but it would have been easy to forget to call this, and authorisation
requires a lot of context from the room.

Another problem was that any endpoint adding events to a room would have to manually construct a
PDU, doing things like finding the latest events to use as `prev_events` and this led to a lot of
boilerplate.

Both of these problems were solved with the StorageExt trait which is implemented for all storage
backends and provides a `fn add_events(&mut self, Event) -> Result<(), Error>` which does
validation, authorisation and finds the latest events.

## 2020-01-18: Storage trait (8bd410e)
Finally the storage API was split off into a trait so it could be implemented in other ways than
with postgres; this was a breath of fresh air because postgres was a pain. Then:

## 2020-02-09: In-memory storage (7782718)
With the new storage API trait I implemented an in-memory storage backend which never saved
anything; it is for testing and having a clean, sane state to use on every server start. It has
allowed debug printing everything at once and the ability to test the server in any environment.
