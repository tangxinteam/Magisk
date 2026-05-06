package com.topjohnwu.magisk.test

import androidx.annotation.Keep
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.topjohnwu.magisk.core.BuildConfig.APP_PACKAGE_NAME
import com.topjohnwu.magisk.core.Const
import com.topjohnwu.magisk.core.model.module.LocalModule
import com.topjohnwu.magisk.core.tasks.AppMigration
import com.topjohnwu.magisk.core.tasks.MagiskInstaller
import com.topjohnwu.magisk.core.utils.RootUtils
import com.topjohnwu.superuser.CallbackList
import com.topjohnwu.superuser.Shell
import com.topjohnwu.superuser.nio.ExtendedFile
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith
import timber.log.Timber
import java.io.PrintStream

@Keep
@RunWith(AndroidJUnit4::class)
class Environment : BaseTest {

    companion object {
        @BeforeClass
        @JvmStatic
        fun before() = BaseTest.prerequisite()

        // It is possible that there are no suitable preinit partition to use
        fun preinit(): Boolean {
            return Shell.cmd("magisk --preinit-device").exec().isSuccess
        }

        private const val MODULE_UPDATE_PATH  = "/data/adb/modules_update"
        const val SEPOLICY_RULE = "sepolicy_rule"
        const val REMOVE_TEST = "remove_test"
        const val REMOVE_TEST_MARKER = "/dev/.remove_test_removed"
        const val UPGRADE_TEST = "upgrade_test"
    }

    object TimberLog : CallbackList<String>(Runnable::run) {
        override fun onAddElement(e: String) {
            Timber.i(e)
        }
    }

    private fun setupSystemlessHost() {
        val error = "hosts setup failed"
        assertTrue(error, runBlocking { RootUtils.addSystemlessHosts() })
        assertTrue(error, RootUtils.fs.getFile(Const.MODULE_PATH).getChildFile("hosts").exists())
    }

    private fun setupSepolicyRuleModule(root: ExtendedFile) {
        val error = "$SEPOLICY_RULE setup failed"
        val path = root.getChildFile(SEPOLICY_RULE)
        assertTrue(error, path.mkdirs())

        // Add sepolicy patch
        PrintStream(path.getChildFile("sepolicy.rule").newOutputStream()).use {
            it.println("type magisk_test domain")
        }

        assertTrue(error, Shell.cmd(
            "set_default_perm $path",
            "copy_preinit_files"
        ).exec().isSuccess)
    }

    private fun setupRemoveModule(root: ExtendedFile) {
        val error = "$REMOVE_TEST setup failed"
        val path = root.getChildFile(REMOVE_TEST)

        // Create a new module but mark is as "remove"
        val module = LocalModule(path)
        assertTrue(error, path.mkdirs())
        // Create uninstaller script
        path.getChildFile("uninstall.sh").newOutputStream().writer().use {
            it.write("touch $REMOVE_TEST_MARKER")
        }
        assertTrue(error, path.getChildFile("service.sh").createNewFile())
        module.remove = true

        assertTrue(error, Shell.cmd("set_default_perm $path").exec().isSuccess)
    }

    private fun setupUpgradeModule(root: ExtendedFile, update: ExtendedFile) {
        val error = "$UPGRADE_TEST setup failed"
        val oldPath = root.getChildFile(UPGRADE_TEST)
        val newPath = update.getChildFile(UPGRADE_TEST)

        // Create an existing module but mark as "disable
        val module = LocalModule(oldPath)
        assertTrue(error, oldPath.mkdirs())
        module.enable = false
        // Install service.sh into the old module
        assertTrue(error, oldPath.getChildFile("service.sh").createNewFile())

        // Create an upgrade module
        assertTrue(error, newPath.mkdirs())
        // Install post-fs-data.sh into the new module
        assertTrue(error, newPath.getChildFile("post-fs-data.sh").createNewFile())

        assertTrue(error, Shell.cmd(
            "set_default_perm $oldPath",
            "set_default_perm $newPath",
        ).exec().isSuccess)
    }

    @Test
    fun setupEnvironment() {
        runBlocking {
            assertTrue(
                "Magisk setup failed",
                MagiskInstaller.Emulator(TimberLog, TimberLog).exec()
            )
        }

        val root = RootUtils.fs.getFile(Const.MODULE_PATH)
        val update = RootUtils.fs.getFile(MODULE_UPDATE_PATH)
        if (preinit()) { setupSepolicyRuleModule(update) }
        setupSystemlessHost()
        setupRemoveModule(root)
        setupUpgradeModule(root, update)
    }

    @Test
    fun setupAppHide() {
        runBlocking {
            assertTrue(
                "App hiding failed",
                AppMigration.patchAndHide(
                    context = appContext,
                    label = "Settings",
                    pkg = "repackaged.$APP_PACKAGE_NAME"
                )
            )
        }
    }

    @Test
    fun setupAppRestore() {
        runBlocking {
            assertTrue(
                "App restoration failed",
                AppMigration.restoreApp(appContext)
            )
        }
    }
}
