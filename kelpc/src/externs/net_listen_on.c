    /* IPv4, bound to the one host the caller named: `127.0.0.1` for loopback,
       `0.0.0.0` (or `*`) for every interface, or a name that resolves to an
       IPv4 address. The cap check ran on `host:port` before this body, so the
       grant has already said this host and port may be listened on. */
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;
    const char* bind_host = kelp_host;
    if (bind_host == NULL || bind_host[0] == '\0' || (bind_host[0] == '*' && bind_host[1] == '\0')) {
        bind_host = NULL;
    }
    char service[8];
    snprintf(service, sizeof service, "%u", (unsigned)kelp_port);
    struct addrinfo* list = NULL;
    int rc = getaddrinfo(bind_host, service, &hints, &list);
    if (rc != 0) {
        kelp_panic_gai("net.listen_on: cannot resolve the bind address", rc);
    }
    int fd = -1;
    int last = 0;
    for (struct addrinfo* a = list; a != NULL; a = a->ai_next) {
        fd = socket(a->ai_family, a->ai_socktype, a->ai_protocol);
        if (fd < 0) {
            last = errno;
            continue;
        }
        int on = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
        if (bind(fd, a->ai_addr, a->ai_addrlen) == 0 && listen(fd, 128) == 0) {
            break;
        }
        last = errno;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(list);
    if (fd < 0) {
        kelp_panic_at("net.listen_on: cannot bind", last);
    }
    return fd;
