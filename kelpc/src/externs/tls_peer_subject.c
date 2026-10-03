    /* The subject of the peer's certificate in RFC 2253 form
       (`CN=client,O=Example`), or `false` when the peer sent none (an
       ordinary server stream without client certificates). */
#if OPENSSL_VERSION_NUMBER >= 0x30000000L
    X509* cert = SSL_get1_peer_certificate((SSL*)kelp_handle);
#else
    X509* cert = SSL_get_peer_certificate((SSL*)kelp_handle);
#endif
    if (cert == NULL) {
        return false;
    }
    BIO* bio = BIO_new(BIO_s_mem());
    if (bio == NULL) {
        X509_free(cert);
        kelp_panic("tls.peer_cert_subject: out of memory");
    }
    X509_NAME_print_ex(bio, X509_get_subject_name(cert), 0, XN_FLAG_RFC2253);
    char* data = NULL;
    long n = BIO_get_mem_data(bio, &data);
    char* text = (char*)malloc((size_t)n + 1);
    if (text == NULL) {
        BIO_free(bio);
        X509_free(cert);
        kelp_panic("tls.peer_cert_subject: out of memory");
    }
    memcpy(text, data, (size_t)n);
    text[n] = 0;
    BIO_free(bio);
    X509_free(cert);
    free(kelp_out->data);
    *kelp_out = kelp_String__from_str(text);
    free(text);
    return true;
