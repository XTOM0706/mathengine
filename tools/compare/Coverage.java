import org.scilab.forge.jlatexmath.*;
import java.nio.file.*;
import java.util.*;

public class Coverage {
    public static void main(String[] a) throws Exception {
        List<String> lines = Files.readAllLines(Paths.get(a[0]));
        TeXFont font = new DefaultTeXFont(10f);
        int ok = 0, no = 0;
        for (String raw : lines) {
            String s = raw.trim();
            if (s.isEmpty()) continue;
            try {
                new TeXFormula(s).createTeXIcon(TeXConstants.STYLE_DISPLAY, 10f);
                System.out.println("OK           " + s);
                ok++;
            } catch (Throwable e) {
                System.out.println("FAIL         " + s + "   [" + e.getClass().getSimpleName() + "]");
                no++;
            }
        }
        System.out.println("--- 成功 " + ok + " / 失败 " + no);
    }
}
