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
  | Author: Wez Furlong <wez@php.net>                                    |
  +----------------------------------------------------------------------+
*/
#include "php_swoole_odbc.h"

#if PHP_VERSION_ID >= 80400 && PHP_VERSION_ID < 80500
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"
#include "zend_exceptions.h"
#include <php_odbc_utils.h>

static const char driver_file[] = __FILE__;
static const int doer_lines[] = {229, 244, 250};
static const int begin_line = 279;
static const int commit_lines[] = {293, 304};
static const int rollback_lines[] = {318, 328};
static const int setup_lines[] = {476, 483, 491, 499, 508, 516};
static const int connect_line = 619;
/* Match the original multiline macro expansion on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 364
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}

static const int prepare_lines[] = {177, 187, 204};

#include "odbc_driver_abi.h"
#endif
