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

#if PHP_VERSION_ID >= 80300 && PHP_VERSION_ID < 80400

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"
#include "zend_exceptions.h"
#include <php_odbc_utils.h>

static const char driver_file[] = __FILE__;
static const int doer_lines[] = {231, 246, 252};
static const int begin_line = 281;
static const int commit_lines[] = {295, 306};
static const int rollback_lines[] = {320, 330};
static const int setup_lines[] = {477, 484, 492, 500, 509, 517};
static const int connect_line = 586;
/* Match the original multiline macro expansion on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 366
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}

static const int prepare_lines[] = {179, 189, 206};

#include "odbc_driver_abi.h"
#endif
