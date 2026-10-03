#include "textures.h"

#include "../modloader-abi/include/modloader_texture.h"
#include "gbi/rt64_f3d.h"
#include "hle/rt64_application.h"
#include "common/rt64_filesystem_zip.h"
#include "xxHash/xxh3.h"
#include "common/rt64_tmem_hasher.h"

#include <fstream>

namespace {

std::vector<std::string> sSources;
std::vector<std::string> sAppliedSources;
int32_t sState = MODLOADER_TEXTURE_DISABLED;

bool Validate(const std::vector<std::string>& sources) {
    for (const auto& source : sources) {
        auto path = std::filesystem::u8path(source);
        json definition;
        if (std::filesystem::is_directory(path)) {
            std::ifstream stream(path / RT64::ReplacementDatabaseFilename, std::ios::binary);
            if (stream.is_open()) {
                definition = json::parse(stream);
            }
        }
        else if (std::filesystem::is_regular_file(path)) {
            auto files = RT64::FileSystemZip::create(path, "");
            std::vector<uint8_t> bytes;
            if (files != nullptr && files->load(RT64::ReplacementDatabaseFilename, bytes)) {
                definition = json::parse(bytes);
            }
        }
        if (!definition.is_object()) {
            fprintf(stderr, "ModLoader: texture source has no readable rt64.json: %s\n", source.c_str());
            return false;
        }
        RT64::ReplacementDatabase database = definition;
        if (database.config.hashVersion > RT64::TMEMHasher::CurrentHashVersion) {
            fprintf(stderr, "ModLoader: unsupported RT64 texture hash version: %s\n", source.c_str());
            return false;
        }
    }
    return true;
}

bool Load(RT64::Application* application, const std::vector<std::string>& sources) {
    std::vector<RT64::ReplacementDirectory> directories;
    directories.reserve(sources.size());
    for (const auto& source : sources) {
        directories.emplace_back(std::filesystem::u8path(source));
    }
    application->workloadQueue->waitForWorkloadId(application->state->workloadId);
    application->presentQueue->waitForPresentId(application->state->presentId);
    application->workloadQueue->waitForIdle();
    application->presentQueue->waitForIdle();
    application->textureCache->waitForGPUUploads();
    return application->textureCache->loadReplacementDirectories(directories);
}

bool Try_Load(RT64::Application* application, const std::vector<std::string>& sources) {
    try {
        return Load(application, sources);
    }
    catch (const std::exception& error) {
        fprintf(stderr, "ModLoader: loading texture sources failed: %s\n", error.what());
        return false;
    }
}

void Apply(RT64::Application* application, bool restore_previous = false) {
    try {
        if (Validate(sSources)) {
            if (application == nullptr) {
                sState = sSources.empty() ? MODLOADER_TEXTURE_DISABLED : MODLOADER_TEXTURE_PENDING;
                if (sSources.empty()) {
                    sAppliedSources.clear();
                }
                return;
            }
            restore_previous = true;
            if (Try_Load(application, sSources)) {
                sAppliedSources = sSources;
                sState = sSources.empty() ? MODLOADER_TEXTURE_DISABLED : MODLOADER_TEXTURE_READY;
                return;
            }
        }
    }
    catch (const std::exception& error) {
        fprintf(stderr, "ModLoader: texture sources failed: %s\n", error.what());
    }
    sState = MODLOADER_TEXTURE_ERROR;
    auto fallback = sAppliedSources;
    std::erase_if(fallback, [](const std::string& source) {
        return std::find(sSources.begin(), sSources.end(), source) == sSources.end();
    });
    if (application != nullptr && (restore_previous || fallback != sAppliedSources)) {
        if (!Try_Load(application, fallback)) {
            fprintf(stderr, "ModLoader: could not restore previous texture sources\n");
            Try_Load(application, {});
            fallback.clear();
        }
    }
    sAppliedSources = std::move(fallback);
}

}

int32_t Texture_Sources_Set(RT64::Application* application, const char* const* paths, uint32_t count, uint32_t flags) {
    if ((count != 0 && paths == nullptr) || (flags & ~MODLOADER_TEXTURE_RELOAD) != 0) {
        return MODLOADER_TEXTURE_ERROR;
    }
    bool unchanged = count == sSources.size();
    for (uint32_t index = 0; index < count; index++) {
        if (paths[index] == nullptr || paths[index][0] == '\0') {
            return MODLOADER_TEXTURE_ERROR;
        }
        unchanged = unchanged && sSources[index] == paths[index];
    }
    if (!unchanged) {
        sSources.clear();
        for (uint32_t index = 0; index < count; index++) {
            sSources.emplace_back(paths[index]);
        }
    }
    if (!unchanged || (flags & MODLOADER_TEXTURE_RELOAD) != 0) {
        Apply(application);
    }
    return sState;
}

void Texture_Sources_Open(RT64::Application* application) {
    if (!sSources.empty()) {
        Apply(application, true);
    }
}

void Texture_Sources_Close() {
    if (sState == MODLOADER_TEXTURE_READY) {
        sState = MODLOADER_TEXTURE_PENDING;
    }
}
