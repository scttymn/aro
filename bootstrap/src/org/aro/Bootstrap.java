package org.aro;

import java.lang.reflect.Method;
import java.lang.reflect.Modifier;

/**
 * In-VM entry point for aro-exec (M1).
 *
 * Runs inside ART, started by app_process64. Loads an unmodified APK through
 * Android's own PathClassLoader and invokes a static method from it. Uses
 * reflection for dalvik.* so it compiles against a plain JDK.
 *
 * Usage: org.aro.Bootstrap <apk> <class> [static-method]
 *        org.aro.Bootstrap <apk> <package-prefix>*     (scan for static no-arg methods)
 *        org.aro.Bootstrap --app                        (run android.app.ActivityThread.main:
 *                                                        the real app entry point; the
 *                                                        Activity service takes it from there)
 */
public final class Bootstrap {
    public static void main(String[] args) throws Exception {
        if (args.length >= 3 && args[0].equals("--call")) {
            // Debug: invoke a static no-arg method on a boot-classpath class and print the result.
            Class<?> c = Class.forName(args[1]);
            Method m = c.getDeclaredMethod(args[2]);
            m.setAccessible(true);
            System.out.println("aro: " + args[1] + "." + args[2] + "() -> " + m.invoke(null));
            return;
        }
        if (args.length >= 1 && args[0].equals("--app")) {
            System.out.println("aro: entering android.app.ActivityThread.main");
            Class<?> at = Class.forName("android.app.ActivityThread");
            at.getMethod("main", String[].class).invoke(null, (Object) new String[0]);
            return;
        }
        if (args.length < 2) {
            System.err.println("usage: org.aro.Bootstrap <apk> <class> [static-method]");
            System.exit(2);
        }
        String apk = args[0];
        String className = args[1];
        String methodName = args.length > 2 ? args[2] : null;

        System.out.println("aro: ART " + System.getProperty("java.vm.version")
                + " on " + System.getProperty("os.arch"));

        ClassLoader parent = Bootstrap.class.getClassLoader();
        Class<?> pcl = Class.forName("dalvik.system.PathClassLoader");
        ClassLoader loader = (ClassLoader) pcl
                .getConstructor(String.class, ClassLoader.class)
                .newInstance(apk, parent);

        if (className.endsWith("*")) {
            scan(apk, loader, className.substring(0, className.length() - 1));
            return;
        }
        Class<?> cls = Class.forName(className, true, loader);
        System.out.println("aro: loaded " + cls.getName() + " from " + apk);

        if (methodName == null) {
            for (Method m : cls.getDeclaredMethods()) {
                if (Modifier.isStatic(m.getModifiers()) && m.getParameterCount() == 0) {
                    System.out.println("aro: static no-arg candidate: " + m.getName());
                }
            }
            return;
        }
        invoke(cls, methodName);
    }

    /** List classes under a package prefix that have public static no-arg methods. */
    private static void scan(String apk, ClassLoader loader, String prefix) throws Exception {
        Class<?> dexFile = Class.forName("dalvik.system.DexFile");
        Object df = dexFile.getConstructor(String.class).newInstance(apk);
        @SuppressWarnings("unchecked")
        java.util.Enumeration<String> names =
                (java.util.Enumeration<String>) dexFile.getMethod("entries").invoke(df);
        int shown = 0;
        while (names.hasMoreElements() && shown < 40) {
            String name = names.nextElement();
            if (!name.startsWith(prefix) || name.contains("$")) continue;
            Class<?> c;
            try {
                c = Class.forName(name, false, loader);
            } catch (Throwable t) {
                continue;
            }
            Method[] methods;
            try {
                methods = c.getDeclaredMethods();
            } catch (Throwable t) {
                continue;
            }
            for (Method m : methods) {
                int mod = m.getModifiers();
                if (Modifier.isStatic(mod) && Modifier.isPublic(mod) && m.getParameterCount() == 0) {
                    System.out.println("aro: candidate " + name + " " + m.getName() + " -> " + m.getReturnType().getSimpleName());
                    shown++;
                }
            }
        }
    }

    private static void invoke(Class<?> cls, String methodName) throws Exception {
        Method m = cls.getDeclaredMethod(methodName);
        m.setAccessible(true);
        Object result = m.invoke(null);
        System.out.println("aro: " + cls.getName() + "." + methodName + "() -> " + result);
    }
}
