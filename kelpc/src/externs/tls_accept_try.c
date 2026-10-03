    /* The server handshake over a socket the caller already accepted. The
       deadline is `SO_RCVTIMEO`/`SO_SNDTIMEO`, as on the client, and a
       `timeout_ms` of 0 is "wait forever". A refused client certificate
       arrives as an X509 verify result (unknown issuer, expired), so that is
       checked first and gets the same codes the client's checks do; every
       other refusal (no certificate, no common ALPN protocol, a version
       below the floor) is a failed handshake. */
    *kelp_err_code = 0;
    SSL* ssl = SSL_new((SSL_CTX*)kelp_ctx);
    if (ssl == NULL) {
        kelp_panic("tls.accept: cannot create an SSL handle");
    }
    if (SSL_set_fd(ssl, kelp_fd) != 1) {
        SSL_free(ssl);
        kelp_panic("tls.accept: cannot bind the socket to the TLS handle");
    }
    if (kelp_timeout_ms > 0) {
        struct timeval tv;
        tv.tv_sec = (time_t)(kelp_timeout_ms / 1000);
        tv.tv_usec = (suseconds_t)((kelp_timeout_ms % 1000) * 1000);
        setsockopt(kelp_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
        setsockopt(kelp_fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    }
    for (;;) {
        int rc = KELP_NO_SIGPIPE(SSL_accept(ssl));
        if (rc == 1) {
            break;
        }
        int err = SSL_get_error(ssl, rc);
        if ((err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE)
                && errno != EAGAIN && errno != EWOULDBLOCK) {
            continue;
        }
        char text[256];
        long v = SSL_get_verify_result(ssl);
        int code = v != X509_V_OK
            ? kelp_tls_classify_verify(v, text, sizeof text)
            : kelp_tls_classify_handshake(ssl, err, text, sizeof text);
        SSL_free(ssl);
        *kelp_err_code = code;
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
        return NULL;
    }
    return ssl;
