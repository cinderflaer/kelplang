    /* Decrypted bytes OpenSSL already holds: the socket may show nothing
       readable while a whole record is waiting here, so a server that waits
       for readability first must ask this. */
    return (size_t)SSL_pending((SSL*)kelp_handle);
