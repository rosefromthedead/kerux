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

## 2019-12-06: Moving to Actix (1774e31)
Tide was bringing problems; it was moving very quickly and I was struggling to even build the
server. It threatened to move to async-std, which meant I couldn't use tokio-postgres, the only
async Postgres driver (at the time). This meant I had to find a new web framework.

### The search for a new web framework
Having decided to move away from Tide, I had to figure out what to move *to*. A few options were
available, and given that Actix was good enough and much more popular than the rest, I decided it
would be the best option. The popularity meant that Actix had been tested and optimised for a
variety of use cases, and was well-ironed.

