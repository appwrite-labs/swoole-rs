/*
  +----------------------------------------------------------------------+
  | Copyright (c) The PHP Group                                          |
  +----------------------------------------------------------------------+
  | This source file is subject to version 3.01 of the PHP license,      |
  | that is bundled with this package in the file LICENSE, and is        |
  | available through the world-wide-web at the following url:           |
  | https://www.php.net/license/3_01.txt                                 |
  | If you did not receive a copy of the PHP license and are unable to   |
  | obtain it through the world-wide-web, please send a note to          |
  | license@php.net so we can mail you a copy immediately.               |
  +----------------------------------------------------------------------+
  | Author: Ard Biesheuvel <abies@php.net>                               |
  +----------------------------------------------------------------------+
*/

#include "php_swoole_firebird.h"


#ifndef _GNU_SOURCE
# define _GNU_SOURCE
#endif

#include "php.h"
#include "zend_exceptions.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "php_pdo_firebird_int.h"
#include "pdo_firebird_utils.h"

/* The statement's allocation, preprocessing and preparing, and its describing, are Rust
 * (crates/firebird/src/prepare.rs); the preparer allocates the record and descriptors between them. */
bool php_firebird_alloc_prepare_stmt(pdo_dbh_t*, const zend_string*, XSQLDA*, isc_stmt_handle*,
	HashTable*);
bool swr_firebird_describe_stmt(pdo_dbh_t *dbh, isc_stmt_handle *s, char *statement_type, XSQLDA *out_sqlda,
	XSQLDA *num_sqlda);
bool swr_firebird_describe_input(pdo_dbh_t *dbh, isc_stmt_handle *s, XSQLDA *in_sqlda, XSQLVAR *vars, short count);
bool php_firebird_begin_transaction(pdo_dbh_t *dbh, bool is_auto_commit_txn);
bool firebird_handle_manually_begin(pdo_dbh_t *dbh);
bool firebird_handle_manually_commit(pdo_dbh_t *dbh);
bool firebird_handle_manually_rollback(pdo_dbh_t *dbh);
bool pdo_firebird_set_attribute(pdo_dbh_t *dbh, zend_long attr, zval *val);
int pdo_firebird_get_attribute(pdo_dbh_t *dbh, zend_long attr, zval *val);
zend_result pdo_firebird_check_liveness(pdo_dbh_t *dbh);
void pdo_firebird_fetch_error_func(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zval *info);
bool pdo_firebird_in_manually_transaction(pdo_dbh_t *dbh);

/* php_firebird_preprocess and its tokenizer are Rust (crates/firebird/src/preprocess.rs),
 * which records each named parameter here. */

void swr_firebird_name_param(HashTable *named_params, const char *name, size_t name_len, zend_long index)
{
	zval tmp;
	ZVAL_LONG(&tmp, index);
	zend_hash_str_update(named_params, name, name_len, &tmp);
}

#if FB_API_VER >= 40
/* set coercing a data type: each variable's is Rust (crates/firebird/src/statement.rs) */
void swr_firebird_coerce(XSQLVAR *var, unsigned client_major);

static void set_coercing_output_data_types(XSQLDA* sqlda)
{
	unsigned fb_client_major_version = (fb_get_client_version() >> 8) & 0xFF;
	for (int i = 0; i < sqlda->sqld; i++) {
		swr_firebird_coerce(&sqlda->sqlvar[i], fb_client_major_version);
	}
}
#endif

/* php_firebird_set_error is Rust (crates/firebird/src/error.rs); it reads the
 * status vector and keeps the message through these. */
const ISC_STATUS *swr_firebird_status_vector(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->isc_status;
}

void swr_firebird_set_einfo(pdo_dbh_t *dbh, int sqlcode, const char *msg, size_t len, bool has_message)
{
	pdo_firebird_error_info *einfo = &((pdo_firebird_db_handle *)dbh->driver_data)->einfo;

	if (einfo->errmsg) {
		pefree(einfo->errmsg, dbh->is_persistent);
	}
	einfo->errmsg = has_message ? pestrndup(msg, len, dbh->is_persistent) : NULL;
	einfo->errmsg_length = has_message ? len : 0;
	einfo->sqlcode = sqlcode;
}

/* called by PDO to close a db handle */
/* The closer is Rust (crates/firebird/src/connection.rs). */
void firebird_handle_closer(pdo_dbh_t *dbh);

/* called by PDO to prepare an SQL query */
static bool firebird_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, /* {{{ */
	pdo_stmt_t *stmt, zval *driver_options)
{
	pdo_firebird_db_handle *H = (pdo_firebird_db_handle *)dbh->driver_data;
	pdo_firebird_stmt *S = NULL;
	HashTable *np;

	do {
		isc_stmt_handle s = PDO_FIREBIRD_HANDLE_INITIALIZER;
		XSQLDA num_sqlda;
		char statement_type;

		num_sqlda.version = PDO_FB_SQLDA_VERSION;
		num_sqlda.sqln = 1;

		ALLOC_HASHTABLE(np);
		zend_hash_init(np, 8, NULL, NULL, 0);

		/* allocate and prepare statement */
		if (!php_firebird_alloc_prepare_stmt(dbh, sql, &num_sqlda, &s, np)) {
			break;
		}

		/* allocate a statement handle struct of the right size (struct out_sqlda is inlined) */
		S = ecalloc(1, sizeof(*S)-sizeof(XSQLDA) + XSQLDA_LENGTH(num_sqlda.sqld));
		S->H = H;
		S->stmt = s;
		S->out_sqlda.version = PDO_FB_SQLDA_VERSION;
		S->out_sqlda.sqln = stmt->column_count = num_sqlda.sqld;
		S->named_params = np;

		/* its type, its output columns and the number of its parameters */
		if (!swr_firebird_describe_stmt(dbh, &s, &statement_type, &S->out_sqlda, &num_sqlda)) {
			break;
		}
		S->statement_type = statement_type;

#if FB_API_VER >= 40
		/* set coercing a data type */
		set_coercing_output_data_types(&S->out_sqlda);
#endif

		/* allocate the input descriptors, all parameters nullable */
		if (num_sqlda.sqld) {
			S->in_sqlda = ecalloc(1,XSQLDA_LENGTH(num_sqlda.sqld));
			S->in_sqlda->version = PDO_FB_SQLDA_VERSION;
			S->in_sqlda->sqln = num_sqlda.sqld;

			if (!swr_firebird_describe_input(dbh, &s, S->in_sqlda, S->in_sqlda->sqlvar, S->in_sqlda->sqln)) {
				break;
			}
		}

		stmt->driver_data = S;
		stmt->methods = &swoole_firebird_stmt_methods;
		stmt->supports_placeholders = PDO_PLACEHOLDER_POSITIONAL;

		return true;

	} while (0);

	php_firebird_error(dbh);

	zend_hash_destroy(np);
	FREE_HASHTABLE(np);

	if (S) {
		if (S->in_sqlda) {
			efree(S->in_sqlda);
		}
		efree(S);
	}

	return false;
}
/* }}} */

/* called by PDO to execute a statement that doesn't produce a result set */
static zend_long firebird_handle_doer(pdo_dbh_t *dbh, const zend_string *sql) /* {{{ */
{
	pdo_firebird_db_handle *H = (pdo_firebird_db_handle *)dbh->driver_data;
	isc_stmt_handle stmt = PDO_FIREBIRD_HANDLE_INITIALIZER;
	static char const info_count[] = { isc_info_sql_records };
	char result[64];
	int ret = 0;
	XSQLDA in_sqlda, out_sqlda;

	/* TODO no placeholders in exec() for now */
	in_sqlda.version = out_sqlda.version = PDO_FB_SQLDA_VERSION;
	in_sqlda.sqld = out_sqlda.sqld = 0;
	out_sqlda.sqln = 1;

	/* allocate and prepare statement */
	if (!php_firebird_alloc_prepare_stmt(dbh, sql, &out_sqlda, &stmt, 0)) {
		return -1;
	}

	/* execute the statement */
	if (isc_dsql_execute2(H->isc_status, &H->tr, &stmt, PDO_FB_SQLDA_VERSION, &in_sqlda, &out_sqlda)) {
		php_firebird_error(dbh);
		ret = -1;
		goto free_statement;
	}

	/* find out how many rows were affected */
	if (isc_dsql_sql_info(H->isc_status, &stmt, sizeof(info_count), const_cast(info_count),
			sizeof(result),	result)) {
		php_firebird_error(dbh);
		ret = -1;
		goto free_statement;
	}

	if (result[0] == isc_info_sql_records) {
		unsigned i = 3, result_size = isc_vax_integer(&result[1],2);

		if (result_size > sizeof(result)) {
			ret = -1;
			goto free_statement;
		}
		while (result[i] != isc_info_end && i < result_size) {
			short len = (short)isc_vax_integer(&result[i+1],2);
			/* bail out on bad len */
			if (len != 1 && len != 2 && len != 4) {
				ret = -1;
				goto free_statement;
			}
			if (result[i] != isc_info_req_select_count) {
				ret += isc_vax_integer(&result[i+3],len);
			}
			i += len+3;
		}
	}

	if (dbh->auto_commit && !H->in_manually_txn) {
		if (!php_firebird_commit_transaction(dbh, /* retain */ true)) {
			ret = -1;
		}
	}

free_statement:

	if (isc_dsql_free_statement(H->isc_status, &stmt, DSQL_drop)) {
		php_firebird_error(dbh);
	}

	return ret;
}
/* }}} */

/* called by the PDO SQL parser to add quotes to values that are copied into SQL */
static zend_string* firebird_handle_quoter(pdo_dbh_t *dbh, const zend_string *unquoted, enum pdo_param_type paramtype)
{
	size_t qcount = 0;
	char const *co, *l, *r;
	char *c;
	size_t quotedlen;
	zend_string *quoted_str;

	if (ZSTR_LEN(unquoted) == 0) {
		return ZSTR_INIT_LITERAL("''", 0);
	}

	/* Firebird only requires single quotes to be doubled if string lengths are used */
	/* count the number of ' characters */
	for (co = ZSTR_VAL(unquoted); (co = strchr(co,'\'')); qcount++, co++);

	if (UNEXPECTED(ZSTR_LEN(unquoted) + 2 > ZSTR_MAX_LEN - qcount)) {
		return NULL;
	}

	quotedlen = ZSTR_LEN(unquoted) + qcount + 2;
	quoted_str = zend_string_alloc(quotedlen, 0);
	c = ZSTR_VAL(quoted_str);
	*c++ = '\'';

	/* foreach (chunk that ends in a quote) */
	for (l = ZSTR_VAL(unquoted); (r = strchr(l,'\'')); l = r+1) {
		strncpy(c, l, r-l+1);
		c += (r-l+1);
		/* add the second quote */
		*c++ = '\'';
	}

	/* copy the remainder */
	strncpy(c, l, quotedlen-(c-ZSTR_VAL(quoted_str))-1);
	ZSTR_VAL(quoted_str)[quotedlen-1] = '\'';
	ZSTR_VAL(quoted_str)[quotedlen]   = '\0';

	return quoted_str;
}
/* }}} */

/* The transactions (php_firebird_begin_transaction, _commit_transaction,
 * _rollback_transaction and firebird_handle_manually_*) are Rust
 * (crates/firebird/src/transaction.rs); they read the connection record through these. */
ISC_STATUS *swr_firebird_status(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->isc_status;
}

isc_db_handle *swr_firebird_db(pdo_dbh_t *dbh)
{
	return &((pdo_firebird_db_handle *)dbh->driver_data)->db;
}

isc_tr_handle *swr_firebird_tr(pdo_dbh_t *dbh)
{
	return &((pdo_firebird_db_handle *)dbh->driver_data)->tr;
}

bool swr_firebird_in_transaction(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->tr != 0;
}

void swr_firebird_txn_options(pdo_dbh_t *dbh, bool *writable, zend_ulong *level)
{
	pdo_firebird_db_handle *H = (pdo_firebird_db_handle *)dbh->driver_data;
	*writable = H->is_writable_txn;
	*level = H->txn_isolation_level;
}

void swr_firebird_set_manual(pdo_dbh_t *dbh, bool manual)
{
	((pdo_firebird_db_handle *)dbh->driver_data)->in_manually_txn = manual;
}

void swr_firebird_error(pdo_dbh_t *dbh)
{
	php_firebird_error(dbh);
}

bool swr_firebird_attached(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->db != 0;
}

void swr_firebird_free(pdo_dbh_t *dbh)
{
	pdo_firebird_db_handle *H = (pdo_firebird_db_handle *)dbh->driver_data;

	if (H->date_format) {
		pefree(H->date_format, dbh->is_persistent);
	}
	if (H->time_format) {
		pefree(H->time_format, dbh->is_persistent);
	}
	if (H->timestamp_format) {
		pefree(H->timestamp_format, dbh->is_persistent);
	}
	if (H->einfo.errmsg) {
		pefree(H->einfo.errmsg, dbh->is_persistent);
	}
	pefree(H, dbh->is_persistent);
}

/* called by PDO to set a driver-specific dbh attribute */
/* pdo_firebird_set_attribute and pdo_firebird_get_attribute are Rust
 * (crates/firebird/src/attribute.rs); they reach the connection record through these. */
#define INFO_BUF_LEN 512

/* callback to used to report database server info */
static void php_firebird_info_cb(void *arg, char const *s) /* {{{ */
{
	if (arg) {
		if (*(char*)arg) { /* second call */
			strlcat(arg, " ", INFO_BUF_LEN);
		}
		strlcat(arg, s, INFO_BUF_LEN);
	}
}
/* }}} */

unsigned short swr_firebird_dialect(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->sql_dialect;
}

bool swr_firebird_in_manual_txn(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->in_manually_txn;
}

void swr_firebird_set_writable(pdo_dbh_t *dbh, bool writable)
{
	((pdo_firebird_db_handle *)dbh->driver_data)->is_writable_txn = writable;
}

void swr_firebird_set_isolation(pdo_dbh_t *dbh, zend_ulong level)
{
	((pdo_firebird_db_handle *)dbh->driver_data)->txn_isolation_level = level;
}

bool swr_firebird_fetch_table_names(pdo_dbh_t *dbh)
{
	return ((pdo_firebird_db_handle *)dbh->driver_data)->fetch_table_names;
}

void swr_firebird_set_fetch_table_names(pdo_dbh_t *dbh, bool on)
{
	((pdo_firebird_db_handle *)dbh->driver_data)->fetch_table_names = on;
}

static char **swr_firebird_format_slot(pdo_firebird_db_handle *H, int which)
{
	return which == 0 ? &H->date_format : which == 1 ? &H->time_format : &H->timestamp_format;
}

const char *swr_firebird_format(pdo_dbh_t *dbh, int which)
{
	static const char *defaults[] = {PDO_FB_DEF_DATE_FMT, PDO_FB_DEF_TIME_FMT, PDO_FB_DEF_TIMESTAMP_FMT};
	char *format = *swr_firebird_format_slot((pdo_firebird_db_handle *)dbh->driver_data, which);
	return format ? format : defaults[which];
}

void swr_firebird_set_format(pdo_dbh_t *dbh, int which, const char *format, size_t len)
{
	char **slot = swr_firebird_format_slot((pdo_firebird_db_handle *)dbh->driver_data, which);
	if (*slot) {
		pefree(*slot, dbh->is_persistent);
		*slot = NULL;
	}
	*slot = pestrndup(format, len, dbh->is_persistent);
}

bool swr_firebird_server_version(pdo_dbh_t *dbh, char *info, size_t len)
{
	(void)len; /* INFO_BUF_LEN */
	*info = 0;
	return !isc_version(&((pdo_firebird_db_handle *)dbh->driver_data)->db, php_firebird_info_cb, (void*)info);
}

bool swr_firebird_connected(pdo_dbh_t *dbh)
{
	return !isc_version(&((pdo_firebird_db_handle *)dbh->driver_data)->db, php_firebird_info_cb, NULL);
}

void swr_firebird_client_version(char *version, size_t len)
{
	(void)len; /* INFO_BUF_LEN */
	isc_get_client_version(version);
}

/* pdo_firebird_check_liveness, pdo_firebird_fetch_error_func and
 * pdo_firebird_in_manually_transaction are Rust (crates/firebird/src/connection.rs). */
size_t swr_firebird_last_error(pdo_dbh_t *dbh, int *sqlcode, char *message, size_t len)
{
	pdo_firebird_error_info *einfo = &((pdo_firebird_db_handle *)dbh->driver_data)->einfo;
	size_t length = einfo->errmsg ? MIN(einfo->errmsg_length, len) : 0;
	*sqlcode = einfo->sqlcode;
	if (length) {
		memcpy(message, einfo->errmsg, length);
	}
	return length;
}

static const struct pdo_dbh_methods firebird_methods = { /* {{{ */
	firebird_handle_closer,
	firebird_handle_preparer,
	firebird_handle_doer,
	firebird_handle_quoter,
	firebird_handle_manually_begin,
	firebird_handle_manually_commit,
	firebird_handle_manually_rollback,
	pdo_firebird_set_attribute,
	NULL, /* last_id not supported */
	pdo_firebird_fetch_error_func,
	pdo_firebird_get_attribute,
	pdo_firebird_check_liveness,
	NULL, /* get driver methods */
	NULL, /* request shutdown */
	pdo_firebird_in_manually_transaction,
	NULL, /* get gc */
	NULL /* scanner */
};
/* }}} */

/* the driver-specific PDO handle constructor */
static int pdo_firebird_handle_factory(pdo_dbh_t *dbh, zval *driver_options) /* {{{ */
{
	struct pdo_data_src_parser vars[] = {
		{ "dbname", NULL, 0 },
		{ "charset",  NULL,	0 },
		{ "role", NULL,	0 },
		{ "dialect", "3", 0 },
		{ "user", NULL, 0 },
		{ "password", NULL, 0 }
	};
	int i, ret = 0;
	short buf_len = 256, dpb_len;

	pdo_firebird_db_handle *H = dbh->driver_data = pecalloc(1,sizeof(*H),dbh->is_persistent);

	php_pdo_parse_data_source(dbh->data_source, dbh->data_source_len, vars, 6);

	if (!dbh->username && vars[4].optval) {
		dbh->username = pestrdup(vars[4].optval, dbh->is_persistent);
	}

	if (!dbh->password && vars[5].optval) {
		dbh->password = pestrdup(vars[5].optval, dbh->is_persistent);
	}

	H->in_manually_txn = false;
	H->is_writable_txn = pdo_attr_lval(driver_options, PDO_FB_WRITABLE_TRANSACTION, 1);
	zend_long txn_isolation_level = pdo_attr_lval(driver_options, PDO_FB_TRANSACTION_ISOLATION_LEVEL, PDO_FB_REPEATABLE_READ);
	if (txn_isolation_level == PDO_FB_READ_COMMITTED ||
		txn_isolation_level == PDO_FB_REPEATABLE_READ ||
		txn_isolation_level == PDO_FB_SERIALIZABLE
	) {
		H->txn_isolation_level = txn_isolation_level;
	} else {
		zend_value_error("Pdo\\Firebird::TRANSACTION_ISOLATION_LEVEL must be a valid transaction isolation level "
			"(Pdo\\Firebird::READ_COMMITTED, Pdo\\Firebird::REPEATABLE_READ, or Pdo\\Firebird::SERIALIZABLE)");
		ret = 0;
	}

	do {
		static char const dpb_flags[] = {
			isc_dpb_user_name, isc_dpb_password, isc_dpb_lc_ctype, isc_dpb_sql_role_name };
		char const *dpb_values[] = { dbh->username, dbh->password, vars[1].optval, vars[2].optval };
		char dpb_buffer[256] = { isc_dpb_version1 }, *dpb;

		dpb = dpb_buffer + 1;

		/* loop through all the provided arguments and set dpb fields accordingly */
		for (i = 0; i < sizeof(dpb_flags); ++i) {
			if (dpb_values[i] && buf_len > 0) {
				dpb_len = slprintf(dpb, buf_len, "%c%c%s", dpb_flags[i], (unsigned char)strlen(dpb_values[i]),
					dpb_values[i]);
				dpb += dpb_len;
				buf_len -= dpb_len;
			}
		}

		H->sql_dialect = PDO_FB_DIALECT;
		if (vars[3].optval) {
			H->sql_dialect = atoi(vars[3].optval);
		}

		/* fire it up baby! */
		if (isc_attach_database(H->isc_status, 0, vars[0].optval, &H->db,(short)(dpb-dpb_buffer), dpb_buffer)) {
			break;
		}

		dbh->methods = &firebird_methods;
		dbh->native_case = PDO_CASE_UPPER;
		dbh->alloc_own_columns = 1;

		ret = 1;

	} while (0);

	for (i = 0; i < sizeof(vars)/sizeof(vars[0]); ++i) {
		if (vars[i].freeme) {
			efree(vars[i].optval);
		}
	}

	if (!dbh->methods) {
		char errmsg[512];
		const ISC_STATUS *s = H->isc_status;
		fb_interpret(errmsg, sizeof(errmsg),&s);
		zend_throw_exception_ex(php_pdo_get_exception(), H->isc_status[1], "SQLSTATE[%s] [%" PRIiPTR "] %s",
				"HY000", H->isc_status[1], errmsg);
	}

	if (ret && dbh->auto_commit && !H->tr) {
		ret = php_firebird_begin_transaction(dbh, /* auto commit mode */ true);
	}

	if (!ret) {
		firebird_handle_closer(dbh);
	}

	return ret;
}
/* }}} */


const pdo_driver_t swoole_pdo_firebird_driver = { /* {{{ */
	PDO_DRIVER_HEADER(firebird),
	pdo_firebird_handle_factory
};
/* }}} */
