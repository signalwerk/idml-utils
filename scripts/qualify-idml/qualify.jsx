#target "indesign"
// Input is copied into a fresh evidence directory by run.sh. No existing user
// document is selected, saved or closed. Rendering is outside this observation.
(function (args) {
    var folder = Folder(args[0]);
    var doc = null;
    var interaction = null;
    var result = {
        applicationVersion: null,
        applicationBuild: null,
        locale: null,
        platform: String($.os),
        pluginInventoryCaptured: false,
        renderingCompared: false,
        opened: false,
        exported: false,
        pageCount: null,
        storyContents: [],
        error: null,
        documentClosed: false,
        interactionRestored: false,
        cleanupErrors: []
    };
    function quote(value) {
        return '"' + String(value).replace(/[\\"\u0000-\u001f]/g, function (c) {
            return "\\u" + ("0000" + c.charCodeAt(0).toString(16)).slice(-4);
        }) + '"';
    }
    function json(value) {
        if (value === null) return "null";
        if (typeof value === "string") return quote(value);
        if (typeof value === "boolean" || typeof value === "number") return String(value);
        var parts = [], key;
        if (value instanceof Array) {
            for (var i = 0; i < value.length; i++) parts.push(json(value[i]));
            return "[" + parts.join(",") + "]";
        }
        for (key in value) if (value.hasOwnProperty(key)) parts.push(quote(key) + ":" + json(value[key]));
        return "{" + parts.join(",") + "}";
    }
    try {
        result.applicationVersion = String(app.version);
        try { result.applicationBuild = String(app.buildNumber); } catch (ignored) {}
        try { result.locale = String(app.locale); } catch (ignoredLocale) {}
        interaction = app.scriptPreferences.userInteractionLevel;
        app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
        doc = app.open(File(folder.fsName + "/input.idml"), false);
        result.opened = true;
        result.pageCount = doc.pages.length;
        for (var i = 0; i < doc.stories.length; i++) result.storyContents.push(String(doc.stories[i].contents));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(folder.fsName + "/resaved.idml"), false);
        result.exported = true;
    } catch (error) {
        result.error = String(error);
    } finally {
        if (doc !== null) {
            try {
                if (doc.isValid) doc.close(SaveOptions.NO);
                result.documentClosed = !doc.isValid;
            } catch (closeError) { result.cleanupErrors.push("Closing test document: " + String(closeError)); }
        }
        if (interaction !== null) {
            try {
                app.scriptPreferences.userInteractionLevel = interaction;
                result.interactionRestored = true;
            } catch (restoreError) { result.cleanupErrors.push("Restoring interaction setting: " + String(restoreError)); }
        }
        var report = File(folder.fsName + "/native-result.json");
        report.encoding = "UTF-8";
        if (!report.open("w")) throw Error("Cannot write native result: " + report.error);
        var written = report.write(json(result) + "\n");
        var closed = report.close();
        if (!written || !closed) throw Error("Cannot finish writing native result: " + report.error);
    }
    if (result.error !== null) throw Error(result.error);
    if (result.cleanupErrors.length) throw Error(result.cleanupErrors.join("; "));
    return folder.fsName;
}(arguments));
