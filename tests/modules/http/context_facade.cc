// The candidate StaticHandler of context_gate.py and conditional_gate.py: a facade over
// the Rust static context (swr_v2_http_context_*) that holds only the server, context
// handle and a borrowed modification-time slot. crates/server/src/static_handler.rs
// drives the same context ABI the same way for the HTTP/1 and HTTP/2 servers. The gates
// slice the class and its definitions from this file; it is never compiled whole.

class StaticHandler {
    Server *serv_;
    swr_v2_http_context *context_;
    const int64_t *mtime_slot_;
    swr_v2_http_observation observe() const;
    std::string text(uint32_t kind) const;
    bool stat_file();
    bool set_filename();
    uint32_t ensure_boundary();
    bool is_modified_range(const std::string &date) const;

  public:
    StaticHandler(Server *server, const char *url, size_t length);
    ~StaticHandler();
    StaticHandler(const StaticHandler &) = delete;
    StaticHandler &operator=(const StaticHandler &) = delete;
    /**
     * @return true: continue to execute backwards
     * @return false: break static handler
     */
    bool try_serve();
    bool try_serve_index_file();
    bool get_dir_files();
    size_t make_index_page(String *buffer);
    static std::string get_date();
    std::string get_date_last_modified() const;
    bool is_modified(const std::string &date) const;
    bool has_index_file() const;
    bool is_enabled_auto_index() const;
    bool is_dir() const;
    void parse_range(const char *range, const char *if_range);
    std::vector<swr_v2_http_static_task> copy_tasks() const;
    size_t get_content_length() const;
    std::string get_mimetype() const;
    const std::string &get_content_type(std::string &scratch);
    size_t get_filesize() const;
    time_t get_file_mtime() const;
    int32_t get_status() const;
    // Copies into native scratch; no Rust pointer escapes.
    void copy_filename(char *output, size_t capacity) const;
    size_t copy_end_part(char *output, size_t capacity) const;
};

std::string StaticHandler::get_date() {
    uint8_t value[64]{};
    const auto length = swr_v3_http_date(nullptr, value, sizeof(value));
    return {reinterpret_cast<const char *>(value), length};
}

StaticHandler::StaticHandler(Server *server, const char *url, size_t length)
    : serv_(server), context_(swr_v2_http_context_new(
        reinterpret_cast<const uint8_t *>(url), length, PATH_MAX)),
      mtime_slot_(swr_v3_http_context_mtime(context_)) {
    if (!context_) throw std::bad_alloc();
}
StaticHandler::~StaticHandler() { swr_v2_http_context_drop(context_); }

swr_v2_http_observation StaticHandler::observe() const {
    swr_v2_http_observation value{};
    if (swr_v2_http_context_observe(context_, &value) < 0) throw std::logic_error("Invalid HTTP context");
    return value;
}
std::string StaticHandler::text(uint32_t kind) const {
    size_t length = 0;
    if (swr_v2_http_context_text(context_, kind, nullptr, 0, &length) < 0)
        throw std::logic_error("Invalid HTTP text query");
    std::string value(length, '\0');
    if (length && swr_v2_http_context_text(context_, kind,
            reinterpret_cast<uint8_t *>(&value[0]), length, &length) < 0)
        throw std::logic_error("Invalid HTTP text copy");
    return value;
}
void StaticHandler::copy_filename(char *output, size_t capacity) const {
    size_t length = 0;
    if (!capacity || swr_v2_http_context_text(context_, SWR_HTTP_CONTEXT_FILENAME,
            reinterpret_cast<uint8_t *>(output), capacity - 1, &length) < 0 || length >= capacity)
        throw std::length_error("Invalid HTTP filename copy");
    output[length] = 0;
}
size_t StaticHandler::copy_end_part(char *output, size_t capacity) const {
    size_t length = 0;
    if (swr_v2_http_context_text(context_, SWR_HTTP_CONTEXT_END,
            reinterpret_cast<uint8_t *>(output), capacity, &length) < 0 || length > capacity)
        throw std::length_error("Invalid HTTP end-part copy");
    return length;
}
int32_t StaticHandler::get_status() const { return observe().status; }
size_t StaticHandler::get_filesize() const { return observe().filesize; }
time_t StaticHandler::get_file_mtime() const { return *mtime_slot_; }
size_t StaticHandler::get_content_length() const { return observe().content_length; }
bool StaticHandler::has_index_file() const {
    size_t length = 0;
    if (swr_v2_http_context_text(context_,SWR_HTTP_CONTEXT_INDEX,nullptr,0,&length) < 0)
        throw std::logic_error("Invalid HTTP index query");
    return length != 0;
}
bool StaticHandler::is_enabled_auto_index() const { return serv_->http_autoindex; }
bool StaticHandler::is_dir() const { auto value=observe(); return value.file_ready && (value.kind & SWR_HTTP_PATH_IS_DIRECTORY); }
std::string StaticHandler::get_mimetype() const {
    char filename[PATH_MAX];
    copy_filename(filename, sizeof(filename));
    return swr_v1_framing_mime_get(reinterpret_cast<const uint8_t *>(filename), strlen(filename));
}
bool StaticHandler::stat_file() { return swr_v3_http_context_stat(context_, nullptr) == 1; }
bool StaticHandler::set_filename() {
    if (swr_v2_http_context_path(context_,2,nullptr,0,0) != 0) return false;
    return stat_file() && (observe().kind & SWR_HTTP_PATH_FILE);
}
uint32_t StaticHandler::ensure_boundary() {
    size_t length=0,count=0;
    if (swr_v2_http_context_text(context_,SWR_HTTP_CONTEXT_BOUNDARY,nullptr,0,&length) < 0)
        throw std::logic_error("Invalid HTTP boundary query");
    if (swr_v1_http_static_boundary(length,&count)) {
        char random[SW_HTTP_SERVER_BOUNDARY_TOTAL_SIZE]{};
        if (count >= sizeof(random)) throw std::length_error("Invalid HTTP boundary RNG span");
        swoole_random_string(random,count);
        if (swr_v2_http_context_boundary(context_,reinterpret_cast<const uint8_t *>(random),count) < 0)
            throw std::bad_alloc();
    }
    return 1;
}
std::string StaticHandler::get_date_last_modified() const {
    uint8_t value[64]{};
    const auto length = swr_v3_http_date(mtime_slot_, value, sizeof(value));
    return {reinterpret_cast<const char *>(value), length};
}
bool StaticHandler::is_modified(const std::string &date) const {
    return swr_v3_http_conditional(date.c_str(), date.size(), 0, mtime_slot_,
        serv_ ? &serv_->timezone_ : nullptr, nullptr) == 1;
}
bool StaticHandler::is_modified_range(const std::string &date) const {
    return swr_v3_http_conditional(date.c_str(), date.size(), 1, mtime_slot_, nullptr, nullptr) == 1;
}

static int32_t resume_static_path(
        swr_v2_http_context *context, const swr_v1_http_proxy_text &root,
        uint32_t event, uint32_t observed, const char *value=nullptr, size_t length=0) {
    swr_v2_http_serve_observation observation{};
    observation.event = event;
    observation.observed = observed;
    observation.value = {reinterpret_cast<const uint8_t *>(value),length};
    return swr_v3_http_context_serve(context,&root,&observation,nullptr);
}

bool StaticHandler::try_serve() {
    swr_v1_server_static_rewrite(serv_->state_handle(), context_);
    const auto &root=serv_->get_document_root();
    const swr_v1_http_proxy_text root_input{reinterpret_cast<const uint8_t *>(root.data()),root.size()};
    swr_v2_http_serve_observation initial{};
    initial.has_locations = !serv_->locations->empty();
    initial.has_index = serv_->http_index_files && !serv_->http_index_files->empty();
    initial.autoindex = serv_->http_autoindex;
    int32_t action = swr_v3_http_context_serve(context_,&root_input,&initial,nullptr);
    auto resume_path = [&](uint32_t event, uint32_t observed, const char *value=nullptr, size_t length=0) {
        return resume_static_path(context_,root_input,event,observed,value,length);
    };
    for (;;) {
        switch (action) {
        case SWR_HTTP_PATH_SKIP: return false;
        case SWR_HTTP_PATH_SERVE: return true;
        case SWR_HTTP_PATH_LOCATIONS:
            // Existing protected configuration container; no mirrored owner.
            for (const auto &location : *serv_->locations)
                swr_v2_http_context_path(context_,1,reinterpret_cast<const uint8_t *>(location.data()),location.size(),0);
            action = resume_path(1,0);
            break;
        case SWR_HTTP_PATH_MIME: {
            char filename[PATH_MAX]; copy_filename(filename,sizeof(filename));
            action = resume_path(4,swr_v1_framing_mime_exists(reinterpret_cast<const uint8_t *>(filename),strlen(filename)));
            break;
        }
        default: throw std::bad_alloc();
        }
    }
}

bool StaticHandler::get_dir_files() {
    const auto result = swr_v3_http_context_directory(context_, nullptr);
    if (result < 0) throw std::bad_alloc();
    return result == 1;
}

bool StaticHandler::try_serve_index_file() {
    size_t cursor = 0;
    int32_t action = swr_v2_http_context_index(context_, 0,
        serv_->http_index_files ? serv_->http_index_files->size() : 0, nullptr, 0, &cursor);
    for (;;) {
        switch (action) {
        case 0: return false;
        case 1: return true;
        case 2: {
            const bool scanned = get_dir_files();
            action = swr_v2_http_context_index(context_, scanned ? 1u : 6u,
                scanned ? serv_->http_index_files->size() : 0, nullptr, 0, &cursor);
            break;
        }
        case 3: {
            const auto &name = (*serv_->http_index_files)[cursor];
            action = swr_v2_http_context_index(context_, 2, 0,
                reinterpret_cast<const uint8_t *>(name.data()), name.size(), &cursor);
            break;
        }
        case 4:
            action = swr_v2_http_context_index(context_, 4, set_filename(), nullptr, 0, &cursor);
            break;
        case 5:
            action = swr_v2_http_context_index(context_, 3, is_enabled_auto_index(), nullptr, 0, &cursor);
            break;
        default: throw std::bad_alloc();
        }
    }
}

static void append_directory_part(String *buffer, const swr_v2_http_context *context, size_t entry, uint32_t part) {
    swr_v1_http_index_page_input input{{nullptr, 0},
        {nullptr, 0},
        {reinterpret_cast<const uint8_t *>(SW_HTTP_PAGE_CSS), sizeof(SW_HTTP_PAGE_CSS) - 1},
        {reinterpret_cast<const uint8_t *>(SW_HTTP_POWER_BY), sizeof(SW_HTTP_POWER_BY) - 1}, part};
    swr_v2_http_page_buffer result{};
    for (;;) {
        switch (swr_v2_http_context_page_emit(context, entry, &input,
                reinterpret_cast<uint8_t *>(buffer->str), buffer->length, buffer->size, &result)) {
        case 1: buffer->reserve(result.required); break;
        case 2: buffer->extend_align(result.required); break;
        case 3: buffer->length = result.length; return;
        default: return;
        }
    }
}

size_t StaticHandler::make_index_page(String *buffer) {
    get_dir_files();
    size_t entry = 0;
    int32_t part = swr_v2_http_context_page_step(context_, 1, &entry);
    while (part > 0) {
        append_directory_part(buffer, context_, entry, part - 1);
        part = swr_v2_http_context_page_step(context_, 0, &entry);
    }
    if (part < 0) throw std::bad_alloc();
    return buffer->length;
}

static int32_t resume_range_header(swr_v2_http_context *owner, uint32_t boundary_ready,
                                   const std::string &mime) {
    swr_v2_http_range_observation observed{{reinterpret_cast<const uint8_t *>(mime.data()),mime.size()},boundary_ready,0};
    return swr_v2_http_context_range_resume(owner,&observed);
}

void StaticHandler::parse_range(const char *range, const char *if_range) {
    static_assert(sizeof(off_t) == sizeof(int64_t), "V2 task offsets require the selected 64-bit native ABI");
    static_assert(SW_HTTP_SERVER_PART_HEADER == 256, "V2 task header capacity changed");
    const size_t length=range ? strlen(range) : 0;
    if (swr_v2_http_context_range_begin(context_,length,if_range != nullptr) < 0)
        throw std::length_error("Invalid HTTP range entry");
    for (;;) {
        const int32_t action=swr_v2_http_context_range_step(context_,reinterpret_cast<const uint8_t *>(range),length);
        if (action == -3) throw std::bad_alloc();
        swr_v2_http_range_observation observed{};
        switch (action) {
        case 0: return;
        case 1: {
            // Preserve the native compiler's paired RNG/MIME argument effect
            // order. Rust retains no Native object or span across those effects.
            const int32_t result=resume_range_header(context_,ensure_boundary(),get_mimetype());
            if (result < 0) throw std::length_error("HTTP range header metadata");
            if (result == 1) errno=EOVERFLOW;
            break;
        }
        case 2:
            observed.boundary_ready=ensure_boundary();
            if (swr_v2_http_context_range_resume(context_,&observed) < 0)
                throw std::bad_alloc();
            break;
        case 3:
            observed.modified=is_modified_range(if_range);
            if (swr_v2_http_context_range_resume(context_,&observed) < 0)
                throw std::logic_error("Invalid HTTP range date observation");
            break;
        default: throw std::logic_error("Invalid HTTP range state");
        }
    }
}

std::vector<swr_v2_http_static_task> StaticHandler::copy_tasks() const {
    const size_t count = observe().task_count;
    std::vector<swr_v2_http_static_task> snapshot(count);
    size_t written = 0;
    if (swr_v2_http_context_tasks(context_, snapshot.data(), snapshot.size(), &written) < 0 || written != count)
        throw std::logic_error("Invalid HTTP task snapshot");
    return snapshot;
}

const std::string &StaticHandler::get_content_type(std::string &scratch) {
    if (swr_v1_http_static_content_prefix(observe().task_count)) {
        ensure_boundary(); // Native RNG ends before owned content formatting.
        if (swr_v2_http_context_format(context_,0) < 0) throw std::bad_alloc();
        scratch=text(SWR_HTTP_CONTEXT_CONTENT);
        return scratch;
    }
    scratch=get_mimetype();
    return scratch;
}

