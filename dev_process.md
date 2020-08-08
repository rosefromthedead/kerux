# Development Process
At the moment, development is driven by Fractal. It is the most spec-compliant client I have found.
 - Riot complains about being unable to make a connection (despite logs showing a successful
 request)
 - RiotX relies on both deprecated features and a recent spec number in /versions
 - Nheko refuses to log in

Meanwhile, Fractal happily logs in, sends and receives messages, and doesn't complain about much
without it being clear what is wrong.

In order to figure out what to do next, a common strategy for me is to load up Fractal and try to
do some stuff, and see where it falls over.

## TLS
As far as I know, all clients require TLS to connect to a server. I do a lot of development at
school so I can't open ports. To get around this, Cloudflare's
[free Argo Tunnel service](https://blog.cloudflare.com/a-free-argo-tunnel-for-your-next-project/)
has been invaluable. It takes HTTPS requests, strips off the encryption,
and sends the requests to a freely available client which relays them to a locally running HTTP
server. This removes any need to obtain and maintain a certificate.
