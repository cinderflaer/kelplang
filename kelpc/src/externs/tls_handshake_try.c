    *kelp_err_code = 0;
    SSL_CTX* ctx = SSL_CTX_new(TLS_client_method());
    if (ctx == NULL) {
        kelp_panic("tls.handshake: cannot create an SSL context");
    }
    SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);
    if (SSL_CTX_set_default_verify_paths(ctx) != 1) {
        SSL_CTX_free(ctx);
        kelp_panic("tls.handshake: cannot load the system trust store");
    }
    /* Private CA certificates arrive as PEM *bytes* the caller read through
       the `fs` gate (`std/tlsfile`); this body never opens a path. They are
       *added* to the roots the call above loaded, never a replacement for
       verifying: nothing on this path lowers SSL_VERIFY_PEER, and the
       hostname check and TLS 1.2 floor below run exactly as they do when no
       CA is given. */
    if (kelp_ca_pem->len > 0) {
        char text[128];
        if (kelp_tls_add_roots(ctx, kelp_ca_pem->data, kelp_ca_pem->len, text, sizeof text) != 0) {
            SSL_CTX_free(ctx);
            *kelp_err_code = 5;
            free(kelp_reason->data);
            *kelp_reason = kelp_String__from_str(text);
            return NULL;
        }
    }
    /* ALPN and a client certificate are the two additions of the extended
       form; with neither given, nothing below changes what this call did
       before. A client certificate is only ever *presented*: it adds nothing
       to the trust decision about the server. */
    int kelp_ext = kelp_alpn->len > 0 || kelp_cert_pem->len > 0;
    if (kelp_alpn->len > 0
            && (!kelp_tls_alpn_valid(kelp_alpn->data, kelp_alpn->len)
                || SSL_CTX_set_alpn_protos(ctx, kelp_alpn->data, (unsigned)kelp_alpn->len) != 0)) {
        SSL_CTX_free(ctx);
        *kelp_err_code = 5;
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str("the ALPN protocol list is not valid");
        return NULL;
    }
    if (kelp_cert_pem->len > 0) {
        char text[128];
        if (kelp_tls_load_chain(ctx, kelp_cert_pem->data, kelp_cert_pem->len, text, sizeof text) != 0
                || kelp_tls_load_key(ctx, kelp_key_pem->data, kelp_key_pem->len, text, sizeof text) != 0) {
            SSL_CTX_free(ctx);
            *kelp_err_code = 5;
            free(kelp_reason->data);
            *kelp_reason = kelp_String__from_str(text);
            return NULL;
        }
    }
    SSL* ssl = SSL_new(ctx);
    /* `ssl` keeps its own reference to `ctx`, so freeing this reference now
       leaves exactly one object (`ssl`) for every failure path below to
       free. */
    SSL_CTX_free(ctx);
    if (ssl == NULL) {
        kelp_panic("tls.handshake: cannot create an SSL handle");
    }
    if (SSL_set_fd(ssl, kelp_fd) != 1) {
        SSL_free(ssl);
        kelp_panic("tls.handshake: cannot bind the socket to the TLS handle");
    }
    /* SO_RCVTIMEO/SO_SNDTIMEO make the blocking SSL_connect give up, which is
       what turns a silent peer into `TlsError.Timeout` instead of a hang. A
       `timeout_ms` of 0 is the socket's own "wait forever", so it is not
       set. */
    if (kelp_timeout_ms > 0) {
        struct timeval tv;
        tv.tv_sec = (time_t)(kelp_timeout_ms / 1000);
        tv.tv_usec = (suseconds_t)((kelp_timeout_ms % 1000) * 1000);
        setsockopt(kelp_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
        setsockopt(kelp_fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    }
    /* SNI and the certificate's hostname check both use the name the caller
       wrote: the same name `std/http.kp`'s capability check ran against,
       not whatever the socket resolved to. */
    SSL_set_tlsext_host_name(ssl, kelp_host);
    if (SSL_set1_host(ssl, kelp_host) != 1) {
        SSL_free(ssl);
        kelp_panic("tls.handshake: cannot set the verification hostname");
    }
    for (;;) {
        int rc = KELP_NO_SIGPIPE(SSL_connect(ssl));
        if (rc == 1) {
            break;
        }
        int err = SSL_get_error(ssl, rc);
        if ((err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE)
                && errno != EAGAIN && errno != EWOULDBLOCK) {
            continue;
        }
        char text[256];
        /* A refused certificate arrives as an SSL error whose text only says
           verification failed; the X509 code is what says whether it was an
           unknown issuer, an expiry or a name mismatch. */
        long v = SSL_get_verify_result(ssl);
        int code = v != X509_V_OK
            ? kelp_tls_classify_verify(v, text, sizeof text)
            : kelp_ext
                ? kelp_tls_classify_handshake(ssl, err, text, sizeof text)
                : kelp_tls_classify_error(ssl, err, text, sizeof text);
        SSL_free(ssl);
        *kelp_err_code = code;
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
        return NULL;
    }
    long verify = SSL_get_verify_result(ssl);
    if (verify != X509_V_OK) {
        char text[256];
        int code = kelp_tls_classify_verify(verify, text, sizeof text);
        SSL_free(ssl);
        *kelp_err_code = code;
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
        return NULL;
    }
    return ssl;
