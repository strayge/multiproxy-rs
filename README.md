# multiproxy-rs

Simple socks5 proxy with multiplexing connection.

## Scheme

```
   Application
        │
        │ socks5
        ▼
   Proxy Client
     │  │  │
     │  │  │ Multiple TCP sessions
     │  │  │
     ▼  ▼  ▼
   Proxy Server
        │
        │
        ▼
    Destination
```
