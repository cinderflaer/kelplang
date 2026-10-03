// The server-side and certificate helpers `std/tls`'s extern bodies share
// (st3). Like the two classifiers before them these are `static inline` and
// use OpenSSL and libc only: this text is emitted before the runtime, so it
// cannot name `kelp_String` or `kelp_Bytes`; the bodies in `externs.rs` do the
// translation to Kelp values.
//
// A private key is never logged and never put in an error text: every key
// problem is reported with one of the fixed sentences below, not with
// OpenSSL's own text for the failing call, and the error queue is cleared
// afterwards so a later call cannot pick the entry up either.

// The result of every `kelp_tls_load_*` and of `kelp_tls_classify_handshake`
// is the code `std/tls`'s `ERR_*` constants name; 5 is `ERR_HANDSHAKE`.
static inline int kelp_tls_classify_handshake(SSL* ssl, int err, char* text, size_t text_len) {
    int code = kelp_tls_classify_error(ssl, err, text, text_len);
    // A refusal the peer (or this side's policy) made during the handshake:
    // an alert, no common ALPN protocol, a missing client certificate. The
    // classifier calls those "io" because it was written for reads and writes;
    // at this point they are a failed handshake.
    if (code == 8 && err == SSL_ERROR_SSL) {
        return 5;
    }
    return code;
}

static inline int kelp_tls_fail(char* text, size_t text_len, const char* message) {
    snprintf(text, text_len, "%s", message);
    ERR_clear_error();
    return 5;
}

// The certificate chain in `pem` (leaf first) as `ctx`'s own. 0, or 5 with a
// fixed sentence in `text`.
static inline int kelp_tls_load_chain(SSL_CTX* ctx, const unsigned char* pem, size_t len, char* text, size_t text_len) {
    if (len == 0 || len > 0x7fffffff) {
        return kelp_tls_fail(text, text_len, "the certificate chain file is empty");
    }
    BIO* bio = BIO_new_mem_buf(pem, (int)len);
    if (bio == NULL) {
        return kelp_tls_fail(text, text_len, "cannot read the certificate chain");
    }
    X509* leaf = PEM_read_bio_X509_AUX(bio, NULL, NULL, NULL);
    if (leaf == NULL) {
        BIO_free(bio);
        return kelp_tls_fail(text, text_len, "the certificate chain is not a valid PEM certificate");
    }
    if (SSL_CTX_use_certificate(ctx, leaf) != 1) {
        X509_free(leaf);
        BIO_free(bio);
        return kelp_tls_fail(text, text_len, "the certificate cannot be used");
    }
    X509_free(leaf);
    for (;;) {
        X509* extra = PEM_read_bio_X509(bio, NULL, NULL, NULL);
        if (extra == NULL) {
            break;
        }
        if (SSL_CTX_add_extra_chain_cert(ctx, extra) != 1) {
            X509_free(extra);
            BIO_free(bio);
            return kelp_tls_fail(text, text_len, "a certificate of the chain cannot be used");
        }
    }
    // The loop ends on the PEM reader's end-of-file entry, which is expected.
    ERR_clear_error();
    BIO_free(bio);
    return 0;
}

// An encrypted key would make OpenSSL ask for a pass phrase on the terminal;
// there is no pass phrase API, so it is refused instead.
static int kelp_tls_no_password(char* buf, int size, int rwflag, void* userdata) {
    (void)buf;
    (void)size;
    (void)rwflag;
    (void)userdata;
    return 0;
}

// The private key in `pem`, which must belong to the certificate already
// loaded into `ctx`. 0, or 5 with a fixed sentence in `text`.
static inline int kelp_tls_load_key(SSL_CTX* ctx, const unsigned char* pem, size_t len, char* text, size_t text_len) {
    if (len == 0 || len > 0x7fffffff) {
        return kelp_tls_fail(text, text_len, "the private key file is empty");
    }
    BIO* bio = BIO_new_mem_buf(pem, (int)len);
    if (bio == NULL) {
        return kelp_tls_fail(text, text_len, "cannot read the private key");
    }
    EVP_PKEY* key = PEM_read_bio_PrivateKey(bio, NULL, kelp_tls_no_password, NULL);
    BIO_free(bio);
    if (key == NULL) {
        return kelp_tls_fail(text, text_len, "the private key is malformed (or encrypted: encrypted keys are not supported)");
    }
    if (SSL_CTX_use_PrivateKey(ctx, key) != 1) {
        EVP_PKEY_free(key);
        return kelp_tls_fail(text, text_len, "the private key does not match the certificate");
    }
    EVP_PKEY_free(key);
    if (SSL_CTX_check_private_key(ctx) != 1) {
        return kelp_tls_fail(text, text_len, "the private key does not match the certificate");
    }
    return 0;
}

// Client certificates are verified against the CAs in `pem` (and only those),
// and those CAs' names are what the server tells the client it will accept.
// 0, or 5 with a fixed sentence.
static inline int kelp_tls_load_client_ca(SSL_CTX* ctx, const unsigned char* pem, size_t len, char* text, size_t text_len) {
    if (len == 0 || len > 0x7fffffff) {
        return kelp_tls_fail(text, text_len, "the client CA file is empty");
    }
    BIO* bio = BIO_new_mem_buf(pem, (int)len);
    if (bio == NULL) {
        return kelp_tls_fail(text, text_len, "cannot read the client CA file");
    }
    X509_STORE* store = SSL_CTX_get_cert_store(ctx);
    int count = 0;
    for (;;) {
        X509* ca = PEM_read_bio_X509(bio, NULL, NULL, NULL);
        if (ca == NULL) {
            break;
        }
        if (X509_STORE_add_cert(store, ca) != 1 || SSL_CTX_add_client_CA(ctx, ca) != 1) {
            X509_free(ca);
            BIO_free(bio);
            return kelp_tls_fail(text, text_len, "a client CA certificate cannot be used");
        }
        X509_free(ca);
        count++;
    }
    ERR_clear_error();
    BIO_free(bio);
    if (count == 0) {
        return kelp_tls_fail(text, text_len, "the client CA file holds no PEM certificate");
    }
    return 0;
}

// ALPN on the server: the protocols (already in the wire format, length-
// prefixed) belong to the `SSL_CTX` through ex_data, so freeing the context
// frees them and nothing outlives it.
typedef struct {
    unsigned char* wire;
    size_t len;
} kelp_alpn_cfg;

static pthread_once_t kelp_alpn_once = PTHREAD_ONCE_INIT;
static int kelp_alpn_idx = -1;

static void kelp_alpn_free(void* parent, void* ptr, CRYPTO_EX_DATA* ad, int idx, long argl, void* argp) {
    (void)parent;
    (void)ad;
    (void)idx;
    (void)argl;
    (void)argp;
    kelp_alpn_cfg* c = (kelp_alpn_cfg*)ptr;
    if (c != NULL) {
        free(c->wire);
        free(c);
    }
}

static void kelp_alpn_init(void) {
    kelp_alpn_idx = SSL_CTX_get_ex_new_index(0, NULL, NULL, NULL, kelp_alpn_free);
}

// The server's choice: the first of its own protocols the client also
// offered. No overlap is a fatal `no_application_protocol` alert.
static int kelp_alpn_select(SSL* ssl, const unsigned char** out, unsigned char* outlen,
                            const unsigned char* in, unsigned int inlen, void* arg) {
    (void)ssl;
    kelp_alpn_cfg* c = (kelp_alpn_cfg*)arg;
    unsigned char* chosen = NULL;
    if (SSL_select_next_proto(&chosen, outlen, c->wire, (unsigned int)c->len, in, inlen)
            == OPENSSL_NPN_NEGOTIATED) {
        *out = chosen;
        return SSL_TLSEXT_ERR_OK;
    }
    return SSL_TLSEXT_ERR_ALERT_FATAL;
}

// Whether `wire` is a well-formed ALPN list: length-prefixed names, none empty,
// none running past the end. `std/tls` writes a zero length for a name that
// cannot be sent (empty or over 255 bytes), so this is what refuses it.
static inline int kelp_tls_alpn_valid(const unsigned char* wire, size_t len) {
    size_t i = 0;
    while (i < len) {
        size_t n = wire[i];
        if (n == 0 || i + 1 + n > len) {
            return 0;
        }
        i += 1 + n;
    }
    return 1;
}

// Builds the server's `SSL_CTX`. `min13` is 1 for a TLS 1.3 floor, 0 for 1.2.
// `ca_len` > 0 turns client certificates on, and then they are required.
// NULL with 5 in `*code` and a fixed sentence in `text` on any failure.
static inline SSL_CTX* kelp_tls_server_ctx(
        const unsigned char* cert, size_t cert_len,
        const unsigned char* key, size_t key_len,
        const unsigned char* ca, size_t ca_len,
        const unsigned char* alpn, size_t alpn_len,
        int min13, int* code, char* text, size_t text_len) {
    *code = 5;
    if (!kelp_tls_alpn_valid(alpn, alpn_len)) {
        kelp_tls_fail(text, text_len, "the ALPN protocol list is not valid");
        return NULL;
    }
    SSL_CTX* ctx = SSL_CTX_new(TLS_server_method());
    if (ctx == NULL) {
        kelp_tls_fail(text, text_len, "cannot create an SSL context");
        return NULL;
    }
    SSL_CTX_set_min_proto_version(ctx, min13 ? TLS1_3_VERSION : TLS1_2_VERSION);
    SSL_CTX_set_options(ctx, SSL_OP_CIPHER_SERVER_PREFERENCE);
    // Required once client certificates are checked, harmless otherwise: it
    // names the application the session cache belongs to.
    SSL_CTX_set_session_id_context(ctx, (const unsigned char*)"kelp-tls", 8);
    if (kelp_tls_load_chain(ctx, cert, cert_len, text, text_len) != 0
            || kelp_tls_load_key(ctx, key, key_len, text, text_len) != 0) {
        SSL_CTX_free(ctx);
        return NULL;
    }
    if (ca_len > 0) {
        if (kelp_tls_load_client_ca(ctx, ca, ca_len, text, text_len) != 0) {
            SSL_CTX_free(ctx);
            return NULL;
        }
        SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT, NULL);
    }
    if (alpn_len > 0) {
        pthread_once(&kelp_alpn_once, kelp_alpn_init);
        kelp_alpn_cfg* c = (kelp_alpn_cfg*)malloc(sizeof *c);
        unsigned char* wire = (unsigned char*)malloc(alpn_len);
        if (c == NULL || wire == NULL || kelp_alpn_idx < 0) {
            free(c);
            free(wire);
            SSL_CTX_free(ctx);
            kelp_tls_fail(text, text_len, "out of memory");
            return NULL;
        }
        memcpy(wire, alpn, alpn_len);
        c->wire = wire;
        c->len = alpn_len;
        if (SSL_CTX_set_ex_data(ctx, kelp_alpn_idx, c) != 1) {
            free(wire);
            free(c);
            SSL_CTX_free(ctx);
            kelp_tls_fail(text, text_len, "cannot attach the ALPN list");
            return NULL;
        }
        SSL_CTX_set_alpn_select_cb(ctx, kelp_alpn_select, c);
    }
    *code = 0;
    return ctx;
}

