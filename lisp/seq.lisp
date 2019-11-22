;;; Forms that work with sequences (list or vectors).

(defn first (obj)
    (if (vec? obj)
        (vec-nth 0 obj)
        (if (list? obj)
            (car obj)
            (err "Not a vector or list"))))

(defn rest (obj)
    (if (vec? obj)
        (vec-slice obj 1)
        (if (list? obj)
            (cdr obj)
            (err "Not a vector or list"))))

(defn last (obj)
    (if (vec? obj)
        (vec-nth (- (length obj) 1) obj)
        (if (list? obj)
            (if (null (cdr obj))
                (car obj)
                (recur (cdr obj)))
            (err "Not a vector or list"))))

(defn butlast (obj)
    (if (vec? obj)
        (vec-slice obj 0 (- (length obj) 1))
        (if (list? obj) (progn
            (defq new-link (join nil nil))
            (if (null (cdr obj))
                (setq new-link nil)
                (setq new-link (join (car obj) (butlast (cdr obj)))))
            new-link)
            (err "Not a vector or list"))))

(defn setnth! (idx obj l)
    (if (vec? l)
        (progn (vec-setnth! idx obj l) nil)
        (if (list? l)
            (if (= idx 0) (progn (xar! l obj) nil) (recur (- idx 1) obj (cdr l)))
            (err "Not a vector or list"))))

(defn nth (idx obj)
    (if (vec? obj)
        (vec-nth idx obj)
        (if (list? obj)
            (if (= idx 0) (car obj) (recur (- idx 1) (cdr obj)))
            (err "Not a vector or list"))))


(def 'append nil)
(def 'append! nil)
(def 'map nil)
(let ((tseq))
    (defn copy-els (to l) (progn
        (def 'tcell nil)
        (for el l
            (if (null to)
                (progn (set 'tseq (set 'to (join el nil))))
                (progn (set 'tcell (join el nil)) (xdr! tseq tcell) (set 'tseq tcell))))
        to))

    (defn last-cell (obj)
        (if (list? obj)
            (if (null (cdr obj))
                obj
                (recur (cdr obj)))
            (err "Not a list")))

    (setfn append (l1 l2 &rest others) (progn
        (def 'ret nil)
        (if (vec? l1)
            (progn
                (set 'ret (make-vec))
                (for el l1 (vec-push! ret el))
                (for el l2 (vec-push! ret el))
                (for l others (for el l (vec-push! ret el))))
            (if (or (list? l1) (null l1))
                (progn
                    (set 'ret (copy-els ret l1))
                    (set 'ret (copy-els ret l2))
                    (for l others
                        (set 'ret (copy-els ret l))))
                (err "First element not a list or vector.")))
        (set 'tseq nil)
        ret))

    (setfn append! (ret l2 &rest others) (progn
        (if (vec? ret)
            (progn
                (for el l2 (vec-push! ret el))
                (for l others (for el l (vec-push! ret el))))
            (if (or (list? ret) (null ret))
                (progn
                    (set 'tseq (last-cell ret))
                    (set 'ret (copy-els ret l2))
                    (for l others
                        (set 'ret (copy-els ret l))))
                (err "First element not a list or vector.")))
        (set 'tseq nil)
        ret))

    (defn map-into (fun items new-items) (progn
        (def 'tcell nil)
        (for i items
            (progn
                (if (null new-items)
                    (progn (set 'tseq (set 'new-items (join (fun i) nil))))
                    (progn (set 'tcell (join (fun i) nil)) (xdr! tseq tcell) (set 'tseq tcell)))))
        new-items))

    (setfn map (fun items)
        (if (vec? items)
            (progn
                (defq new-items (make-vec (length items)))
                (for i items (vec-push! new-items (fun i)))
                new-items)
            (if (list? items)
                (progn
                    (defq new-items nil)
                    (set 'new-items (map-into(fun items new-items)))
                    (set 'tseq nil)
                    new-items)
                (if (null items)
                    nil
                    (err "Not a list or vector"))))))

(defn map! (fun items) (progn
    (fori i it items
        (setnth! i (fun it) items))
    items))

(defn reverse (items) (progn
    (if (vec? items)
        (progn
            (defn irev (items new-items num)
                (if (>= num 0) (progn (vec-push! new-items (nth num items))(recur items new-items (- num 1)))))
            (defq new-items (make-vec (length items)))
            (irev items new-items (- (length items) 1))
            new-items)
        (if (list? items)
            (progn
                (def 'titems (copy-seq items))
                (reverse! titems))
            (if (null items)
                nil
                (err "Not a list or vector."))))))

(defn reverse! (items) (progn

    (defn irev (items first last)
        (if (> last first) (progn
            (defq ftemp (nth first items))
            (setnth! first (nth last items) items)
            (setnth! last ftemp items)
            (recur items (+ first 1) (- last 1)))))

    (irev items 0 (- (length items) 1))
    items))

(ns-export '(first rest last butlast setnth! nth append append! map map! reverse reverse!))
