#target "indesign"
// Creates one separate, unsaved document; never selects or modifies an existing
// document. The launcher supplies a newly created evidence directory.
(function (args) {
    var folder = Folder(args[0]), doc = null, interaction = null;
    var result = {
        applicationVersion: null, applicationBuild: null, locale: null,
        platform: String($.os), pluginInventoryCaptured: false,
        renderingCompared: false, created: false, exported: false,
        pageCount: null, storyContents: [], frameBounds: null,
        requestedPageDimensions: ["612pt", "792pt"],
        requestedFrameBounds: ["36pt", "36pt", "200pt", "400pt"],
        documentClosed: false, interactionRestored: false,
        error: null, cleanupErrors: []
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
        doc = app.documents.add(false);
        result.created = true;
        doc.documentPreferences.pagesPerDocument = 1;
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = "612pt";
        doc.documentPreferences.pageHeight = "792pt";
        var frame = doc.pages.item(0).textFrames.add();
        frame.geometricBounds = result.requestedFrameBounds;
        frame.contents = "idml-utils native baseline";
        result.pageCount = doc.pages.length;
        for (var i = 0; i < doc.stories.length; i++) result.storyContents.push(String(doc.stories[i].contents));
        var bounds = frame.geometricBounds;
        result.frameBounds = [Number(bounds[0]), Number(bounds[1]), Number(bounds[2]), Number(bounds[3])];
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(folder.fsName + "/baseline.idml"), false);
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
        var written = report.write(json(result) + "\n"), closed = report.close();
        if (!written || !closed) throw Error("Cannot finish writing native result: " + report.error);
    }
    if (result.error !== null) throw Error(result.error);
    if (result.cleanupErrors.length) throw Error(result.cleanupErrors.join("; "));
    return folder.fsName;
}(arguments));
