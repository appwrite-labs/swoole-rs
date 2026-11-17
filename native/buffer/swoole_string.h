/* SPDX-License-Identifier: Apache-2.0 */
/* Rust owns String; native socket records only retain its pointer. */
#pragma once

namespace swoole {
class String;
}  // namespace swoole
