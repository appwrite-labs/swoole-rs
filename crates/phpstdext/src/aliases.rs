//! The functions methods on arrays, strings and streams call (upstream's
//! `array_methods`, `string_methods` and `stream_methods`).
use std::ffi::CStr;

/// What a method is called on.
#[derive(Clone, Copy)]
pub(crate) enum Receiver {
    Array,
    String,
    Stream,
}

/// The function `name` calls on `receiver`, if it is a method.
pub(crate) fn find(receiver: Receiver, name: &[u8]) -> Option<&'static CStr> {
    let table: &[(&[u8], &CStr)] = match receiver {
        Receiver::Array => ARRAY,
        Receiver::String => STRING,
        Receiver::Stream => STREAM,
    };
    table
        .iter()
        .find(|(method, _)| *method == name)
        .map(|(_, function)| *function)
}

const ARRAY: &[(&[u8], &CStr)] = &[
    (b"all", c"array_all"),
    (b"any", c"array_any"),
    (b"changeKeyCase", c"array_change_key_case"),
    (b"chunk", c"array_chunk"),
    (b"column", c"array_column"),
    (b"countValues", c"array_count_values"),
    (b"diff", c"array_diff"),
    (b"diffAssoc", c"array_diff_assoc"),
    (b"diffKey", c"array_diff_key"),
    (b"filter", c"array_filter"),
    (b"find", c"array_find"),
    (b"flip", c"array_flip"),
    (b"intersect", c"array_intersect"),
    (b"intersectAssoc", c"array_intersect_assoc"),
    (b"isList", c"array_is_list"),
    (b"keyExists", c"swoole_array_key_exists"),
    (b"keyFirst", c"array_key_first"),
    (b"keyLast", c"array_key_last"),
    (b"keys", c"array_keys"),
    (b"map", c"swoole_array_map"),
    (b"pad", c"array_pad"),
    (b"product", c"array_product"),
    (b"rand", c"array_rand"),
    (b"reduce", c"array_reduce"),
    (b"replace", c"array_replace"),
    (b"reverse", c"array_reverse"),
    (b"search", c"swoole_array_search"),
    (b"slice", c"array_slice"),
    (b"sum", c"array_sum"),
    (b"unique", c"array_unique"),
    (b"values", c"array_values"),
    (b"count", c"count"),
    (b"merge", c"array_merge"),
    (b"contains", c"swoole_array_contains"),
    (b"join", c"swoole_array_join"),
    (b"isTyped", c"swoole_array_is_typed"),
    (b"isEmpty", c"swoole_array_is_empty"),
    // pass by ref
    (b"sort", c"sort"),
    (b"pop", c"array_pop"),
    (b"push", c"array_push"),
    (b"shift", c"array_shift"),
    (b"unshift", c"array_unshift"),
    (b"splice", c"array_splice"),
    (b"walk", c"array_walk"),
    (b"replaceStr", c"swoole_array_replace_str"),
    (b"iReplaceStr", c"swoole_array_ireplace_str"),
    // serialize
    (b"serialize", c"serialize"),
    (b"marshal", c"serialize"),
    (b"jsonEncode", c"json_encode"),
];

/// i=ignore case, l=left, r=right
const STRING: &[(&[u8], &CStr)] = &[
    (b"length", c"strlen"),
    (b"isEmpty", c"swoole_str_is_empty"),
    (b"lower", c"strtolower"),
    (b"upper", c"strtoupper"),
    (b"lowerFirst", c"lcfirst"),
    (b"upperFirst", c"ucfirst"),
    (b"upperWords", c"ucwords"),
    (b"addCSlashes", c"addcslashes"),
    (b"addSlashes", c"addslashes"),
    (b"chunkSplit", c"chunk_split"),
    (b"countChars", c"count_chars"),
    (b"htmlEntityDecode", c"html_entity_decode"),
    (b"htmlEntityEncode", c"htmlentities"),
    (b"htmlSpecialCharsEncode", c"htmlspecialchars"),
    (b"htmlSpecialCharsDecode", c"htmlspecialchars_decode"),
    (b"trim", c"trim"),
    (b"lTrim", c"ltrim"),
    (b"rTrim", c"rtrim"),
    (b"parseStr", c"swoole_parse_str"),
    (b"parseUrl", c"parse_url"),
    (b"contains", c"str_contains"),
    (b"incr", c"str_increment"),
    (b"decr", c"str_decrement"),
    (b"pad", c"str_pad"),
    (b"repeat", c"str_repeat"),
    (b"replace", c"swoole_str_replace"),
    (b"iReplace", c"swoole_str_ireplace"),
    (b"shuffle", c"str_shuffle"),
    (b"split", c"swoole_str_split"), // explode
    (b"startsWith", c"str_starts_with"),
    (b"endsWith", c"str_ends_with"),
    (b"wordCount", c"str_word_count"),
    (b"iCompare", c"strcasecmp"),
    (b"compare", c"strcmp"),
    (b"find", c"strstr"),
    (b"iFind", c"stristr"),
    (b"stripTags", c"strip_tags"),
    (b"stripCSlashes", c"stripcslashes"),
    (b"stripSlashes", c"stripslashes"),
    (b"iIndexOf", c"stripos"),
    (b"indexOf", c"strpos"),
    (b"lastIndexOf", c"strrpos"),
    (b"iLastIndexOf", c"strripos"),
    (b"lastCharIndexOf", c"strrchr"),
    (b"substr", c"substr"),
    (b"substrCompare", c"substr_compare"),
    (b"substrCount", c"substr_count"),
    (b"substrReplace", c"substr_replace"),
    (b"reverse", c"strrev"),
    (b"md5", c"md5"),
    (b"sha1", c"sha1"),
    (b"crc32", c"crc32"),
    (b"hash", c"swoole_hash"),
    (b"hashCode", c"swoole_hashcode"),
    (b"base64Decode", c"base64_decode"),
    (b"base64Encode", c"base64_encode"),
    (b"urlDecode", c"urldecode"),
    (b"urlEncode", c"urlencode"),
    (b"rawUrlEncode", c"rawurlencode"),
    (b"rawUrlDecode", c"rawurldecode"),
    (b"match", c"swoole_str_match"),
    (b"matchAll", c"swoole_str_match_all"),
    (b"isNumeric", c"is_numeric"),
    // mbstring
    (b"mbUpperFirst", c"mb_ucfirst"),
    (b"mbLowerFirst", c"mb_lcfirst"),
    (b"mbTrim", c"mb_trim"),
    (b"mbSubstrCount", c"mb_substr_count"),
    (b"mbSubstr", c"mb_substr"),
    (b"mbUpper", c"mb_strtoupper"),
    (b"mbLower", c"mb_strtolower"),
    (b"mbFind", c"mb_strstr"),
    (b"mbIndexOf", c"mb_strpos"),
    (b"mbLastIndexOf", c"mb_strrpos"),
    (b"mbILastIndexOf", c"mb_strripos"),
    (b"mbLastCharIndexOf", c"mb_strrchr"),
    (b"mbILastCharIndex", c"mb_strrichr"),
    (b"mbLength", c"mb_strlen"),
    (b"mbIFind", c"mb_stristr"),
    (b"mbIIndexOf", c"mb_stripos"),
    (b"mbCut", c"mb_strcut"),
    (b"mbRTrim", c"mb_rtrim"),
    (b"mbLTrim", c"mb_ltrim"),
    (b"mbDetectEncoding", c"mb_detect_encoding"),
    (b"mbConvertEncoding", c"mb_convert_encoding"),
    (b"mbConvertCase", c"mb_convert_case"),
    // serialize
    (b"unserialize", c"unserialize"),
    (b"unmarshal", c"unserialize"),
    (b"jsonDecode", c"swoole_str_json_decode"),
    (b"jsonDecodeToObject", c"swoole_str_json_decode_to_object"),
];

const STREAM: &[(&[u8], &CStr)] = &[
    (b"write", c"fwrite"),
    (b"read", c"fread"),
    (b"close", c"fclose"),
    (b"dataSync", c"fdatasync"),
    (b"sync", c"fsync"),
    (b"truncate", c"ftruncate"),
    (b"stat", c"fstat"),
    (b"seek", c"fseek"),
    (b"tell", c"ftell"),
    (b"lock", c"flock"),
    (b"eof", c"feof"),
    (b"getChar", c"fgetc"),
    (b"getLine", c"fgets"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_name_their_functions() {
        assert_eq!(find(Receiver::Array, b"map"), Some(c"swoole_array_map"));
        assert_eq!(find(Receiver::String, b"split"), Some(c"swoole_str_split"));
        assert_eq!(find(Receiver::Stream, b"getLine"), Some(c"fgets"));
        assert_eq!(find(Receiver::Stream, b"map"), None);
        assert_eq!(
            find(Receiver::Array, b"MAP"),
            None,
            "names are case-sensitive"
        );
        assert_eq!(find(Receiver::Array, b"map\0"), None, "the whole name");
    }

    #[test]
    fn tables_have_upstreams_entries() {
        assert_eq!((ARRAY.len(), STRING.len(), STREAM.len()), (49, 89, 13));
        for table in [ARRAY, STRING, STREAM] {
            for (index, (method, _)) in table.iter().enumerate() {
                assert!(
                    table[..index].iter().all(|(other, _)| other != method),
                    "duplicate method {}",
                    String::from_utf8_lossy(method)
                );
            }
        }
    }
}
