import org.scilab.forge.jlatexmath.*;
import java.util.*;

public class Fuzz {
    static final String[] T = {
        "x","y","a","1","2","+","-","=","^","_","{","}","\\frac","\\sqrt","\\sum","\\int",
        "\\alpha","\\left(","\\right)","\\binom","\\begin{pmatrix}","\\end{pmatrix}","&","\\\\",
        "\\hat","\\text","\\,","\\unknowncmd","中"," ","(",")","[","]","\\lim","\\infty","\\partial","\\to"
    };
    static long state;
    static long next() {
        state = state * 6364136223846793005L + 1442695040888963407L;
        return state >>> 33;
    }
    static String randLatex() {
        int n = (int)(next() % 24);
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < n; i++) {
            sb.append(T[(int)(next() % T.length)]);
            if (next() % 4 == 0) sb.append(' ');
        }
        return sb.toString();
    }
    public static void main(String[] a) throws Exception {
        state = 0x5EED0706L;
        TeXFont font = new DefaultTeXFont(10f);
        int ok = 0, err = 0;
        Map<String,Integer> kinds = new TreeMap<>();
        List<String> samples = new ArrayList<>();
        for (int i = 0; i < 2000; i++) {
            String src = randLatex();
            try {
                new TeXFormula(src).createTeXIcon(TeXConstants.STYLE_DISPLAY, 10f);
                ok++;
            } catch (Throwable e) {
                err++;
                String k = e.getClass().getSimpleName();
                kinds.merge(k, 1, Integer::sum);
                if (samples.size() < 3) samples.add(src + "  =>  " + k);
            }
        }
        System.out.println("成功 " + ok + " 条，异常 " + err + " 条");
        System.out.println("异常类型分布: " + kinds);
        for (String s : samples) System.out.println("  例: " + s);
    }
}
