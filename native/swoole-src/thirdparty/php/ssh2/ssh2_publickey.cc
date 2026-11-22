/*
   +----------------------------------------------------------------------+
   | PHP Version 7                                                        |
   +----------------------------------------------------------------------+
   | Copyright (c) 1997-2016 The PHP Group                                |
   +----------------------------------------------------------------------+
   | This source file is subject to version 3.01 of the PHP license,      |
   | that is bundled with this package in the file LICENSE, and is        |
   | available through the world-wide-web at the following url:           |
   | http://www.php.net/license/3_01.txt                                  |
   | If you did not receive a copy of the PHP license and are unable to   |
   | obtain it through the world-wide-web, please send a note to          |
   | license@php.net so we can mail you a copy immediately.               |
   +----------------------------------------------------------------------+
   | Author: Sara Golemon <pollita@php.net>                               |
   +----------------------------------------------------------------------+
*/

/* ext/ssh2's publickey subsystem, the only ssh2 PHP functions left in C: whether
 * they keep C's behaviour (libssh2_publickey_* without the coroutine hook, so
 * ssh2_publickey_init never gets past EAGAIN) or are retried through it is the
 * open decision on #5517. The rest of ext/ssh2 is crates/ssh2/src/php.rs and,
 * for what stays C, native/hooks/ssh2*.cc. */

#include "php_ssh2.h"
#include "php_swoole_ssh2_hook.h"

/* ***********************
 * Publickey Subsystem *
 *********************** */

/* {{{ proto resource ssh2_publickey_init(resource connection)
Initialize the publickey subsystem */
PHP_FUNCTION(ssh2_publickey_init) {
    zval *zsession;
    LIBSSH2_SESSION *session;
    LIBSSH2_PUBLICKEY *pkey;
    php_ssh2_pkey_subsys_data *data;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "r", &zsession) == FAILURE) {
        return;
    }

    SSH2_FETCH_AUTHENTICATED_SESSION(session, zsession);

    pkey = libssh2_publickey_init(session);

    if (!pkey) {
        int last_error = 0;
        char *error_msg = NULL;

        last_error = libssh2_session_last_error(session, &error_msg, NULL, 0);
        php_error_docref(NULL, E_WARNING, "Unable to initialize publickey subsystem(%d) %s", last_error, error_msg);
        RETURN_FALSE;
    }

    data = (php_ssh2_pkey_subsys_data *) emalloc(sizeof(php_ssh2_pkey_subsys_data));
    data->session = session;
    data->session_rsrc = Z_RES_P(zsession);
    Z_ADDREF_P(zsession);
    data->pkey = pkey;

    RETURN_RES(zend_register_resource(data, le_ssh2_pkey_subsys));
}
/* }}} */

/* {{{ proto bool ssh2_publickey_add(resource pkey, string algoname, string blob[, bool overwrite=FALSE [,array
attributes=NULL]]) Add an additional publickey */
PHP_FUNCTION(ssh2_publickey_add) {
    zval *zpkey_data, *zattrs = NULL;
    php_ssh2_pkey_subsys_data *data;
    char *algo, *blob;
    size_t algo_len, blob_len;
    unsigned long num_attrs = 0;
    libssh2_publickey_attribute *attrs = NULL;
    zend_bool overwrite = 0;

    if (zend_parse_parameters(
            ZEND_NUM_ARGS(), "rss|ba", &zpkey_data, &algo, &algo_len, &blob, &blob_len, &overwrite, &zattrs) ==
        FAILURE) {
        return;
    }

    if ((data = (php_ssh2_pkey_subsys_data *) zend_fetch_resource(
             Z_RES_P(zpkey_data), PHP_SSH2_PKEY_SUBSYS_RES_NAME, le_ssh2_pkey_subsys)) == NULL) {
        RETURN_FALSE;
    }

    if (zattrs) {
        HashPosition pos;
        zval *attr_val;
        unsigned long current_attr = 0;

        num_attrs = zend_hash_num_elements(Z_ARRVAL_P(zattrs));
        attrs = (libssh2_publickey_attribute *) safe_emalloc(num_attrs, sizeof(libssh2_publickey_attribute), 0);

        for (zend_hash_internal_pointer_reset_ex(Z_ARRVAL_P(zattrs), &pos);
             (attr_val = zend_hash_get_current_data_ex(Z_ARRVAL_P(zattrs), &pos)) != NULL;
             zend_hash_move_forward_ex(Z_ARRVAL_P(zattrs), &pos)) {
            zend_string *key;
            int type;
            zend_ulong idx;
            zval copyval = *attr_val;

            type = zend_hash_get_current_key_ex(Z_ARRVAL_P(zattrs), &key, &idx, &pos);
            if (type == HASH_KEY_NON_EXISTENT) {
                /* All but impossible */
                break;
            }
            if (type == HASH_KEY_IS_LONG) {
                /* Malformed, ignore */
                php_error_docref(NULL, E_WARNING, "Malformed attirbute array, contains numeric index");
                num_attrs--;
                continue;
            }

            if (!key || (key->len == 1 && key->val[0] == '*')) {
                /* Empty key, ignore */
                php_error_docref(NULL, E_WARNING, "Empty attribute key");
                num_attrs--;
                continue;
            }

            zval_copy_ctor(&copyval);
            // TODO Sean-Der
            // Z_UNSET_ISREF_P(&copyval);
            // Z_SET_REFCOUNT_P(&copyval, 1);
            convert_to_string(&copyval);

            if (key->val[0] == '*') {
                attrs[current_attr].mandatory = 1;
                attrs[current_attr].name = key->val + 1;
                attrs[current_attr].name_len = key->len - 2;
            } else {
                attrs[current_attr].mandatory = 0;
                attrs[current_attr].name = key->val;
                attrs[current_attr].name_len = key->len - 1;
            }
            attrs[current_attr].value_len = Z_STRLEN(copyval);
            attrs[current_attr].value = Z_STRVAL(copyval);

            /* copyval deliberately not dtor'd, we're stealing the string */
            current_attr++;
        }
    }

    if (libssh2_publickey_add_ex(data->pkey,
                                 (unsigned char *) algo,
                                 algo_len,
                                 (unsigned char *) blob,
                                 blob_len,
                                 overwrite,
                                 num_attrs,
                                 attrs)) {
        php_error_docref(NULL, E_WARNING, "Unable to add %s key", algo);
        RETVAL_FALSE;
    } else {
        RETVAL_TRUE;
    }

    if (attrs) {
        unsigned long i;

        for (i = 0; i < num_attrs; i++) {
            /* name doesn't need freeing */
            // TODO Sean-Der
            // efree(attrs[i].value);
        }
        efree(attrs);
    }
}
/* }}} */

/* {{{ proto bool ssh2_publickey_remove(resource pkey, string algoname, string blob)
Remove a publickey entry */
PHP_FUNCTION(ssh2_publickey_remove) {
    zval *zpkey_data;
    php_ssh2_pkey_subsys_data *data;
    char *algo, *blob;
    size_t algo_len, blob_len;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "rss", &zpkey_data, &algo, &algo_len, &blob, &blob_len) == FAILURE) {
        return;
    }

    if ((data = (php_ssh2_pkey_subsys_data *) zend_fetch_resource(
             Z_RES_P(zpkey_data), PHP_SSH2_PKEY_SUBSYS_RES_NAME, le_ssh2_pkey_subsys)) == NULL) {
        RETURN_FALSE;
    }

    if (libssh2_publickey_remove_ex(data->pkey, (unsigned char *) algo, algo_len, (unsigned char *) blob, blob_len)) {
        php_error_docref(NULL, E_WARNING, "Unable to remove %s key", algo);
        RETURN_FALSE;
    }

    RETURN_TRUE;
}
/* }}} */

/* {{{ proto array ssh2_publickey_list(resource pkey)
List currently installed publickey entries */
PHP_FUNCTION(ssh2_publickey_list) {
    zval *zpkey_data;
    php_ssh2_pkey_subsys_data *data;
    unsigned long num_keys, i;
    libssh2_publickey_list *keys;
    zend_string *hash_key_zstring;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "r", &zpkey_data) == FAILURE) {
        return;
    }

    if ((data = (php_ssh2_pkey_subsys_data *) zend_fetch_resource(
             Z_RES_P(zpkey_data), PHP_SSH2_PKEY_SUBSYS_RES_NAME, le_ssh2_pkey_subsys)) == NULL) {
        RETURN_FALSE;
    }

    if (libssh2_publickey_list_fetch(data->pkey, &num_keys, &keys)) {
        php_error_docref(NULL, E_WARNING, "Unable to list keys on remote server");
        RETURN_FALSE;
    }

    array_init(return_value);
    for (i = 0; i < num_keys; i++) {
        zval key, attrs;
        unsigned long j;

        array_init(&key);

        add_assoc_stringl(&key, "name", (char *) keys[i].name, keys[i].name_len);
        add_assoc_stringl(&key, "blob", (char *) keys[i].blob, keys[i].blob_len);

        array_init(&attrs);
        for (j = 0; j < keys[i].num_attrs; j++) {
            zval attr;

            ZVAL_STRINGL(&attr, keys[i].attrs[j].value, keys[i].attrs[j].value_len);
            hash_key_zstring = zend_string_init(keys[i].attrs[j].name, keys[i].attrs[j].name_len, 0);
            zend_hash_add(Z_ARRVAL_P(&attrs), hash_key_zstring, &attr);
            zend_string_release(hash_key_zstring);
        }
        add_assoc_zval(&key, "attrs", &attrs);

        add_next_index_zval(return_value, &key);
    }

    libssh2_publickey_list_free(data->pkey, keys);
}
/* }}} */

/* ssh2_auth_agent is Rust (crates/ssh2/src/php.rs). */

