/* PDO method signatures, Zend allocation and native statement record access.
 * Each supported PHP driver supplies its preparer and original diagnostic locations. */
extern void swr_odbc_fetch_error(pdo_dbh_t *, pdo_stmt_t *, zval *);

/* The native statement still owns its record. Rust reads the same error
 * layout for connection and statement diagnostics. */
_Static_assert(SQL_MAX_MESSAGE_LENGTH == 512, "ODBC diagnostic buffer layout");
pdo_odbc_errinfo *swr_odbc_error_record(pdo_dbh_t *dbh, pdo_stmt_t *stmt) {
    if (stmt) {
        return &((pdo_odbc_stmt *) stmt->driver_data)->einfo;
    }
    return &((pdo_odbc_db_handle *) dbh->driver_data)->einfo;
}

SQLHENV swr_odbc_environment(pdo_dbh_t *dbh) {
    return ((pdo_odbc_db_handle *) dbh->driver_data)->env;
}

SQLHSTMT swr_odbc_statement(pdo_stmt_t *stmt) {
    return ((pdo_odbc_stmt *) stmt->driver_data)->stmt;
}

void swr_odbc_connect_exception(pdo_dbh_t *dbh) {
    const pdo_odbc_errinfo *error = swr_odbc_error_record(dbh, NULL);
    zend_throw_exception_ex(php_pdo_get_exception(), error->last_error, "SQLSTATE[%s] %s: %d %s",
                            dbh->error_code, error->what, error->last_error, error->last_err_msg);
}

extern void swr_odbc_close(pdo_dbh_t *, SQLHENV);

void swr_odbc_free_record(pdo_dbh_t *dbh) {
    pefree(dbh->driver_data, dbh->is_persistent);
    dbh->driver_data = NULL;
}

static void odbc_handle_closer(pdo_dbh_t *dbh) {
    pdo_odbc_db_handle *handle = (pdo_odbc_db_handle *) dbh->driver_data;
    swr_odbc_close(dbh, handle->env);
}

extern zend_long swr_odbc_doer(pdo_dbh_t *, const zend_string *, const char *, int, int, int);

static zend_long odbc_handle_doer(pdo_dbh_t *dbh, const zend_string *sql) {
    return swr_odbc_doer(dbh, sql, driver_file, doer_lines[0], doer_lines[1], doer_lines[2]);
}

SQLHDBC swr_odbc_connection(pdo_dbh_t *dbh) {
    return ((pdo_odbc_db_handle *)dbh->driver_data)->dbc;
}

extern bool swr_odbc_begin(pdo_dbh_t *, const char *, int);
extern bool swr_odbc_finish(pdo_dbh_t *, bool, const char *, int, int);

static bool odbc_handle_begin(pdo_dbh_t *dbh) {
    return swr_odbc_begin(dbh, driver_file, begin_line);
}
static bool odbc_handle_commit(pdo_dbh_t *dbh) {
    return swr_odbc_finish(dbh, true, driver_file, commit_lines[0], commit_lines[1]);
}
static bool odbc_handle_rollback(pdo_dbh_t *dbh) {
    return swr_odbc_finish(dbh, false, driver_file, rollback_lines[0], rollback_lines[1]);
}

void swr_odbc_set_utf8(pdo_dbh_t *dbh, bool on) {
    ((pdo_odbc_db_handle *) dbh->driver_data)->assume_utf8 = on;
}

extern bool swr_odbc_set_attr(pdo_dbh_t *, zend_long, zval *, const char *, int);
static bool odbc_handle_set_attr(pdo_dbh_t *dbh, zend_long attr, zval *value) {
    return swr_odbc_set_attr(dbh, attr, value, driver_file, attribute_line());
}

extern int swr_odbc_get_attr(pdo_dbh_t *, zend_long, zval *, bool, const char *);
extern zend_result swr_odbc_check_liveness(pdo_dbh_t *);

static int odbc_handle_get_attr(pdo_dbh_t *dbh, zend_long attr, zval *value) {
    pdo_odbc_db_handle *handle = (pdo_odbc_db_handle *) dbh->driver_data;
    return swr_odbc_get_attr(dbh, attr, value, handle->assume_utf8, "ODBC-" PDO_ODBC_TYPE);
}

static const struct pdo_dbh_methods odbc_methods = {
    .closer = odbc_handle_closer,
    .preparer = odbc_handle_preparer,
    .doer = odbc_handle_doer,
    .begin = odbc_handle_begin,
    .commit = odbc_handle_commit,
    .rollback = odbc_handle_rollback,
    .set_attribute = odbc_handle_set_attr,
    .fetch_err = swr_odbc_fetch_error,
    .get_attribute = odbc_handle_get_attr,
    .check_liveness = swr_odbc_check_liveness,
};

extern bool swr_odbc_connect(pdo_dbh_t *, const char *, size_t, const char *, const char *, const char *, int);
void swr_odbc_replace_source(pdo_dbh_t *dbh, const char *source) {
    size_t length = strlen(source);
    char *copy = pestrndup(source, length, dbh->is_persistent);
    pefree((char *) dbh->data_source, dbh->is_persistent);
    dbh->data_source = copy;
#if PHP_VERSION_ID >= 80500
    dbh->data_source_len = length;
#endif
}

extern bool swr_odbc_setup(pdo_dbh_t *, zval *, SQLHENV *, SQLHDBC *, const char *, const int (*)[6]);

static int swr_odbc_factory(pdo_dbh_t *dbh, zval *driver_options) {
    pdo_odbc_db_handle *H = pecalloc(1, sizeof(*H), dbh->is_persistent);
    dbh->driver_data = H;
    bool connected = swr_odbc_setup(dbh, driver_options, &H->env, &H->dbc, driver_file, &setup_lines)
        && swr_odbc_connect(dbh, dbh->data_source, dbh->data_source_len + 1,
                           dbh->username, dbh->password, driver_file, connect_line);
    dbh->methods = &odbc_methods;
    if (connected) {
        dbh->alloc_own_columns = 1;
    }
    return connected;
}

/* PDO_DRIVER_HEADER takes the API version from the PHP headers used to build this module. */
const pdo_driver_t *swr_odbc_descriptor(void) {
    static const pdo_driver_t driver = {PDO_DRIVER_HEADER(odbc), swr_odbc_factory};
    return &driver;
}
